"""5 个 Gateway 工具的行为（owner: T3）。

头等大事是 `read_group_history` 的过滤：§3.2 里 `PlatformPort.read_history` 明写
「不做 sender_kind 过滤（过滤归 Gateway 的 read_group_history 工具）」，
§6 的 T3 行也把它列成独有验收。假平台照契约把 bot / app / system 消息原样交上来，
所以这里过不了就是真漏了，不是替身太宽松。
"""
from aite.contracts import ExecResult, FileEntry, SenderKind, ToolCallRequest


def req(name: str, **arguments) -> ToolCallRequest:
    return ToolCallRequest(call_id="call-1", name=name, arguments=arguments)


# --- read_group_history ------------------------------------------------------

async def test_read_group_history_drops_every_non_human_sender(gateway, ctx, history):
    result = await gateway.call(ctx, req("read_group_history"))

    assert result.ok is True, result.error
    kinds = {m["sender_kind"] for m in result.data["messages"]}
    assert kinds == {SenderKind.human.value}

    humans = [m for m in history if m.sender_kind == SenderKind.human.value]
    others = [m for m in history if m.sender_kind != SenderKind.human.value]
    assert result.data["count"] == len(humans)
    assert result.data["filtered_out"] == len(others)

    # content 里也不许留下机器人说过的话
    for message in others:
        assert message.message_id not in result.content
        assert message.text not in result.content
    for message in humans:
        assert message.message_id in result.content
        assert message.text in result.content


async def test_read_group_history_content_uses_the_w1_line_format(gateway, ctx):
    """与 §3.6 W1 的群历史窗口一致：[message_id] 姓名: 文本。"""
    result = await gateway.call(ctx, req("read_group_history"))
    assert "[om_1] 张三: 这周的退款单据我整理好了" in result.content


async def test_read_group_history_passes_limit_through(gateway, ctx, platform):
    await gateway.call(ctx, req("read_group_history", limit=2))

    name, args, kwargs = platform.calls[-1]
    assert name == "read_history"
    assert args == (ctx.chat_id,)
    assert kwargs == {"limit": 2, "thread_id": None}


async def test_read_group_history_thread_only_scopes_to_the_thread(gateway, ctx, platform):
    result = await gateway.call(ctx, req("read_group_history", thread_only=True))

    assert platform.calls[-1][2]["thread_id"] == ctx.thread_id
    ids = [m["message_id"] for m in result.data["messages"]]
    assert ids == ["om_1", "om_3"]          # om_6 在话题外，om_2/om_4 不是真人


async def test_read_group_history_thread_only_without_a_thread_is_honest(gateway, ctx, platform):
    """顶层消息没有 thread_id：如实说没有话题历史，而不是悄悄把全群历史端上来。"""
    toplevel = ctx.model_copy(update={"thread_id": None})
    result = await gateway.call(toplevel, req("read_group_history", thread_only=True))

    assert result.ok is True
    assert result.data == {"messages": [], "count": 0, "filtered_out": 0, "thread_only": True}
    assert platform.calls == []             # 压根没去问平台


async def test_read_group_history_with_only_bots_says_so(gateway, ctx, platform):
    platform.history = [m for m in platform.history if m.sender_kind != SenderKind.human.value]
    result = await gateway.call(ctx, req("read_group_history"))

    assert result.ok is True
    assert result.data["count"] == 0
    assert result.data["filtered_out"] == 3
    assert "没有可引用的真人消息" in result.content


# --- read_document -----------------------------------------------------------

async def test_read_document_returns_markdown(gateway, ctx):
    result = await gateway.call(
        ctx, req("read_document", url_or_token="https://feishu.cn/docx/abc")
    )

    assert result.ok is True, result.error
    assert result.data["title"] == "退款流程 SOP"
    assert "# 退款流程 SOP" in result.content
    assert "1. 核对单据" in result.content


async def test_read_document_unknown_ref_is_upstream(gateway, ctx):
    result = await gateway.call(ctx, req("read_document", url_or_token="https://feishu.cn/docx/nope"))

    assert result.ok is False
    assert result.error.code == "upstream"


# --- download_attachment -----------------------------------------------------

async def test_download_attachment_lands_in_work_in(gateway, ctx, sandbox, platform):
    result = await gateway.call(ctx, req("download_attachment", file_key="file_v3_csv"))

    assert result.ok is True, result.error
    assert result.data["path"] == "/work/in/file_v3_csv"
    assert result.data["size"] == len(platform.files["file_v3_csv"])

    sandbox_id = gateway.sandbox_id_of(ctx.task_id)
    assert await sandbox.get_file(sandbox_id, "/work/in/file_v3_csv") == platform.files["file_v3_csv"]


async def test_download_attachment_uses_the_message_from_ctx(gateway, ctx, platform):
    await gateway.call(ctx, req("download_attachment", file_key="file_v3_csv"))

    name, args, _kwargs = platform.calls[-1]
    assert (name, args) == ("download_file", (ctx.attachments_message_id, "file_v3_csv"))


async def test_download_attachment_sanitizes_the_file_key(gateway, ctx, platform):
    """file_key 是平台给的不透明串，直接当路径用会爬出 /work/in。"""
    platform.files["../../etc/passwd"] = b"nope"
    result = await gateway.call(ctx, req("download_attachment", file_key="../../etc/passwd"))

    assert result.ok is True, result.error
    assert result.data["path"] == "/work/in/passwd"


async def test_download_attachment_without_attachment_context_is_upstream(gateway, ctx):
    bare = ctx.model_copy(update={"attachments_message_id": None})
    result = await gateway.call(bare, req("download_attachment", file_key="file_v3_csv"))

    assert result.ok is False
    assert result.error.code == "upstream"


# --- run_python --------------------------------------------------------------

async def test_run_python_execs_in_the_sandbox(gateway, ctx, sandbox):
    result = await gateway.call(ctx, req("run_python", code="print('hi')", timeout_sec=30))

    assert result.ok is True, result.error
    sandbox_id, exec_req = sandbox.exec_requests[-1]
    assert sandbox_id == gateway.sandbox_id_of(ctx.task_id)
    assert (exec_req.code, exec_req.timeout_sec, exec_req.language) == ("print('hi')", 30, "python")
    assert "fake stdout" in result.content


async def test_run_python_reports_files_out_as_artifacts(gateway, ctx, sandbox):
    sandbox.exec_result = ExecResult(
        exit_code=0,
        stdout="saved",
        stderr="",
        duration_ms=42,
        files_out=[FileEntry(path="/work/out.png", size=2048)],
    )

    result = await gateway.call(ctx, req("run_python", code="..."))

    assert [a.path for a in result.artifacts] == ["/work/out.png"]
    assert result.artifacts[0].title == "out.png"
    assert result.artifacts[0].mime == "image/png"
    assert result.data["files_out"] == [{"path": "/work/out.png", "size": 2048}]


async def test_run_python_user_error_is_not_a_tool_failure(gateway, ctx, sandbox):
    """代码自己报错 → ok=True + traceback 进 content。

    算成 code=sandbox 会撞上 §3.3 的「连续 2 次沙箱失败 → task failed」，
    两个语法错就把任务打死了。"""
    sandbox.exec_result = ExecResult(
        exit_code=1,
        stdout="",
        stderr="Traceback (most recent call last):\nValueError: 列名不对",
        duration_ms=15,
    )

    result = await gateway.call(ctx, req("run_python", code="df['nope']"))

    assert result.ok is True
    assert result.error is None
    assert result.data["exit_code"] == 1
    assert "ValueError: 列名不对" in result.content


async def test_run_python_touches_the_sandbox(gateway, ctx, sandbox):
    """§3.6 W7 的 reaper 每 60s 扫一次，任务跑一半不能被收走。"""
    await gateway.call(ctx, req("run_python", code="print(1)"))
    assert sandbox.touched == [gateway.sandbox_id_of(ctx.task_id)]


async def test_run_python_flags_truncated_output(gateway, ctx, sandbox):
    sandbox.exec_result = ExecResult(
        exit_code=0, stdout="x" * 100, stderr="", duration_ms=1, truncated=True
    )
    result = await gateway.call(ctx, req("run_python", code="print(1)"))

    assert result.data["truncated"] is True
    assert "截断" in result.content


# --- list_files --------------------------------------------------------------

async def test_list_files_without_a_sandbox_does_not_create_one(gateway, ctx, sandbox):
    """模型常在跑代码前先问一句「有什么文件」，为这一问起个容器纯属浪费。"""
    result = await gateway.call(ctx, req("list_files"))

    assert result.ok is True, result.error
    assert result.data == {"files": [], "count": 0}
    assert gateway.sandbox_id_of(ctx.task_id) is None
    assert sandbox.trees == {}


async def test_list_files_lists_the_work_tree(gateway, ctx, sandbox):
    await gateway.call(ctx, req("download_attachment", file_key="file_v3_csv"))
    await gateway.call(ctx, req("run_python", code="open('/work/out.txt','w').write('x')"))

    result = await gateway.call(ctx, req("list_files"))

    assert result.data["files"] == ["/work/in/file_v3_csv", "/work/out.txt"]
    assert "/work/out.txt" in result.content


# --- 沙箱的生命周期（一个 task 一个容器）------------------------------------

async def test_one_sandbox_per_task_is_reused(gateway, ctx, sandbox):
    await gateway.call(ctx, req("run_python", code="print(1)"))
    first = gateway.sandbox_id_of(ctx.task_id)
    await gateway.call(ctx, req("run_python", code="print(2)"))

    assert gateway.sandbox_id_of(ctx.task_id) == first
    assert len(sandbox.trees) == 1


async def test_different_tasks_get_different_sandboxes(gateway, ctx, sandbox):
    await gateway.call(ctx, req("run_python", code="print(1)"))

    other = ctx.model_copy(update={"task_id": "task-two", "session_token": "a" * 32})
    gateway.register_task("task-two", "a" * 32)
    await gateway.call(other, req("run_python", code="print(2)"))

    assert gateway.sandbox_id_of(ctx.task_id) != gateway.sandbox_id_of("task-two")
    assert len(sandbox.trees) == 2


async def test_release_task_drops_the_sandbox_and_the_token(gateway, ctx, sandbox):
    """!stop / 任务收尾时用（§3.3）。幂等。"""
    await gateway.call(ctx, req("run_python", code="print(1)"))
    sandbox_id = gateway.sandbox_id_of(ctx.task_id)

    await gateway.release_task(ctx.task_id)
    await gateway.release_task(ctx.task_id)

    assert sandbox.released == [sandbox_id]
    assert gateway.sandbox_id_of(ctx.task_id) is None
    # token 也撤了：同一个 task 再来调工具就该被拒
    assert (await gateway.call(ctx, req("list_files"))).error.code == "denied"


async def test_token_resolver_can_replace_the_registry(make_gateway, ctx):
    """TΩ 组装时把它接到 SessionStore.get_task().session_token 上（Gateway 不认识 T2 的 store）。"""
    tokens = {ctx.task_id: ctx.session_token}
    gateway = make_gateway(register=False, token_resolver=tokens.get)

    assert (await gateway.call(ctx, req("list_files"))).ok is True
    tokens.clear()
    assert (await gateway.call(ctx, req("list_files"))).error.code == "denied"
