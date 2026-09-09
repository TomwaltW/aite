"""W3 的 Answering 路径、W5 的产物交付、W8 的证据落盘。"""
import hashlib
import json

from worker_fakes import final_turn, tool_turn

from aite.contracts import EvidenceKind, TaskStatus

PNG = b"\x89PNG\r\n\x1a\n" + b"fake"


# ---- W3：第一步就 final → 不发卡片 ---------------------------------------


async def test_answering_path_sends_text_only(run_task, platform):
    _, task, model = await run_task([final_turn("北京今天 26 度。")])

    assert platform.cards == []                     # 没有卡片
    assert platform.card_updates == []
    assert len(platform.texts) == 1
    assert platform.texts[0].text == "北京今天 26 度。"
    assert task.status is TaskStatus.delivered
    assert len(model.calls) == 1


async def test_plain_text_on_first_step_is_treated_as_final(run_task, platform):
    """§3.3 对国内模型的兜底：第一步没调工具、只回文本 → 当作 final。"""
    from worker_fakes import text_turn

    _, task, _ = await run_task([text_turn("直接回答你：42。")])

    assert platform.cards == []
    assert platform.texts[0].text == "直接回答你：42。"
    assert task.status is TaskStatus.delivered


# ---- W5：产物 -------------------------------------------------------------


async def test_final_artifacts_are_sent_into_the_thread(run_task, platform, sandbox, store, evidence):
    sandbox.files["/work/out.png"] = PNG
    script = [
        tool_turn(("run_python", {"code": "..."})),
        final_turn("图在这里。", [{"path": "/work/out.png", "title": "月度趋势"}]),
    ]

    _, task, _ = await run_task(script, step_seconds=0.6)
    session = await store.get_session(task.session_id)

    assert len(platform.files) == 1
    sent = platform.files[0]
    assert sent.data == PNG
    assert sent.name == "out.png"
    assert sent.mime == "image/png"
    assert sent.reply_to == session.anchor.thread_id == "om_1"   # 回到话题 root

    # 文件先发，再发正文
    assert platform.texts[-1].text == "图在这里。"
    assert task.status is TaskStatus.delivered

    kinds = [e["kind"] for e in _events(evidence, task.id)]
    assert kinds.count(EvidenceKind.artifact.value) == 1
    art = next(e for e in _events(evidence, task.id) if e["kind"] == EvidenceKind.artifact.value)
    assert art["payload"] == {
        "title": "月度趋势",
        "mime": "image/png",
        "sha256": hashlib.sha256(PNG).hexdigest(),
        "size": len(PNG),
    }


async def test_missing_artifact_is_skipped_but_task_still_delivered(run_task, platform, sandbox):
    """§3.3：path 不在 /work 下或不存在 → 跳过该产物，回帖附一行，任务仍 delivered。"""
    sandbox.files["/work/ok.txt"] = b"hi"
    script = [
        tool_turn(("list_files", {})),
        final_turn(
            "两个产物。",
            [
                {"path": "/work/ok.txt", "title": "好的"},
                {"path": "/work/missing.png", "title": "缺的"},
                {"path": "/tmp/outside.png", "title": "越界的"},
            ],
        ),
    ]

    _, task, _ = await run_task(script, step_seconds=0.6)

    assert [f.name for f in platform.files] == ["ok.txt"]
    body = platform.texts[-1].text
    assert body.startswith("两个产物。")
    assert "产物 缺的 未找到" in body
    assert "产物 越界的 未找到" in body
    assert task.status is TaskStatus.delivered


async def test_final_without_reply_is_invalid_args(run_task, platform):
    script = [tool_turn(("final", {"artifacts": []})), final_turn("这次带上了正文")]
    _, task, _ = await run_task(script, step_seconds=0.6)

    assert task.status is TaskStatus.delivered
    assert platform.texts[-1].text == "这次带上了正文"


# ---- W8：证据 -------------------------------------------------------------


def _events(evidence, task_id) -> list[dict]:
    path = evidence.events_path(task_id)
    return [json.loads(x) for x in path.read_text(encoding="utf-8").splitlines() if x.strip()]


async def test_evidence_chain_covers_the_run(run_task, evidence, store, gateway):
    script = [
        tool_turn(("checklist_add", {"items": ["取数"]})),
        tool_turn(("run_python", {"code": "print(1)"})),
        tool_turn(("checklist_check", {"id": "c1"})),
        final_turn("好了"),
    ]
    _, task, _ = await run_task(script, step_seconds=0.6)

    kinds = [e["kind"] for e in _events(evidence, task.id)]
    assert kinds[0] == EvidenceKind.task_created.value
    assert kinds[1] == EvidenceKind.event_received.value
    assert kinds.count(EvidenceKind.model_call.value) == 4
    assert kinds.count(EvidenceKind.checklist_op.value) == 2
    assert kinds.count(EvidenceKind.tool_call.value) == 4        # 含 final
    # final 成功时没有 tool_result：它不给模型返回结果，任务到此为止，由 delivered 那条承接
    assert kinds.count(EvidenceKind.tool_result.value) == 3
    assert kinds[-1] == EvidenceKind.delivered.value

    assert evidence.verify(task.id) is True
    manifest = json.loads(evidence.manifest_path(task.id).read_text(encoding="utf-8"))
    assert manifest["root_hash"] == task.evidence_root_hash
    assert manifest["task_no"] == task.task_no
    assert manifest["session_id"] == task.session_id
    assert manifest["event_count"] == len(kinds)


async def test_model_message_text_never_enters_evidence(run_task, evidence):
    """W8：模型消息全文不进 evidence，只进 transcript。"""
    secret = "这是模型写的一段很长的正文，绝不该出现在证据里"
    _, task, _ = await run_task([final_turn(secret)])

    raw = evidence.events_path(task.id).read_text(encoding="utf-8")
    model_calls = [e for e in _events(evidence, task.id) if e["kind"] == EvidenceKind.model_call.value]
    assert model_calls and set(model_calls[0]["payload"]) == {
        "model", "step", "messages_hash", "usage", "finish_reason",
    }
    # final 的 arguments 里确实有正文（那是工具调用参数，不是模型消息全文），
    # 但 model_call 那条只留 hash
    assert secret not in json.dumps(model_calls[0], ensure_ascii=False)
    assert raw.count(secret) == 1


async def test_gateway_tool_result_is_recorded_as_hash(run_task, evidence, gateway):
    script = [tool_turn(("read_document", {"url_or_token": "doc1"})), final_turn("读完了")]
    _, task, _ = await run_task(script, step_seconds=0.6)

    results = [e for e in _events(evidence, task.id) if e["kind"] == EvidenceKind.tool_result.value]
    payload = results[0]["payload"]
    assert set(payload) == {"call_id", "name", "ok", "error", "content_hash", "duration_ms"}
    assert payload["name"] == "read_document" and payload["ok"] is True
    assert payload["content_hash"] == hashlib.sha256(b"read_document ok").hexdigest()
    assert len(gateway.calls) == 1


async def test_gateway_gets_the_task_session_token(run_task, gateway, store):
    script = [tool_turn(("list_files", {})), final_turn("好")]
    _, task, _ = await run_task(script, step_seconds=0.6)

    ctx, req = gateway.calls[0]
    assert ctx.session_token == task.session_token and len(task.session_token) == 32
    assert ctx.task_id == task.id
    assert ctx.session_id == task.session_id
    assert ctx.thread_id == "om_1"
    assert req.name == "list_files"
