"""第 2 组：evidence 真落盘（§2.2 B7 的文件版）。

B7 验的是 §3.1 的 hash 测试向量与「篡改 payload → verify False」，用的是直接调
`FileEvidenceWriter`。这里换成**一个真跑完的任务**：证据是 worker / control plane
在真实时序里一条条追加出来的，然后：

* 逐行重算整条链（用 §3.1 的 `payload_hash_of` / `chain_hash`，不借 `verify` 自证）；
* `manifest.json` 的 `root_hash` / `event_count` 与文件对得上；
* 库里 `Task.evidence_root_hash` 与盘上的 manifest 对得上（两处口径不许漂）；
* 再走一遍现成的 `verify`；
* **文件里每一行的 payload 逐个改坏，每一次都必须 verify False**。
"""
import json

from app_under_test import build_app
from integration_fakes import (
    CHAT,
    GatedPlatform,
    RecordingModel,
    events_path,
    final_step,
    make_event,
    manifest_path,
    read_events,
    read_manifest,
    read_task_from_disk,
    running_app,
    tool_step,
    wait_until,
)

from aite.contracts import (
    CONTRACT_VERSION,
    GENESIS,
    EvidenceKind,
    TaskStatus,
    chain_hash,
    payload_hash_of,
)
from aite.evidence import FileEvidenceWriter
from aite.testing import FakeSandbox

#: 一条会写出丰富证据链的脚本：checklist → run_python（真过 Gateway 和沙箱）
#: → checklist_check → final 带产物。
RICH_SCRIPT = [
    tool_step("checklist_add", {"items": ["取数", "画图", "交付"]}),
    tool_step(
        "run_python",
        {"code": "import matplotlib\nplt.savefig('/work/out.png')\n", "timeout_sec": 30},
    ),
    tool_step("checklist_check", {"id": "c1"}),
    final_step("月度趋势图见附件。", artifacts=[{"path": "/work/out.png", "title": "趋势图"}]),
]

EXEC_SCRIPT = [
    {
        "match": "savefig",
        "exit_code": 0,
        "stdout": "saved /work/out.png",
        "writes": {"/work/out.png": "builtin:png"},
    }
]


async def _run_one_task(config) -> tuple[str, str]:
    """跑完一个多步任务，返回 (task_id, session_id)。"""
    platform = GatedPlatform()
    app = build_app(
        config,
        platform=platform,
        model=RecordingModel(RICH_SCRIPT),
        sandbox=FakeSandbox(exec_script=EXEC_SCRIPT),
    )
    async with running_app(app):
        await platform.emit(make_event(text="把这个 CSV 画成月度趋势图"))
        task = (await app.store.list_active_tasks(CHAT))[0]
        await wait_until(lambda: platform.count("send_text") == 1, what="任务交付")
        assert platform.count("send_file") == 1, "final.artifacts 应当走 W5 发回线程"
        session_id = task.session_id
    return task.id, session_id


async def test_events_jsonl_and_manifest_are_real_files(config):
    task_id, session_id = await _run_one_task(config)

    path = events_path(config, task_id)
    assert path.is_file(), f"{path} 应当存在"
    lines = path.read_text(encoding="utf-8").splitlines()
    assert lines, "任务跑完了却一条证据都没有"
    assert all(line.strip() for line in lines), "events.jsonl 不许有空行"

    events = read_events(config, task_id)
    assert len(events) == len(lines)                             # 一行一个事件
    assert [e.seq for e in events] == list(range(len(events)))   # seq 从 0 连续
    assert all(e.task_id == task_id for e in events)

    # 链条在**文件里**重算一遍。不调 verify —— 那是被验的对象，不能拿它自证。
    prev = GENESIS
    for e in events:
        assert e.payload is not None, f"seq={e.seq} 的 payload 没内联，本用例的载荷都远小于 64KB"
        assert e.payload_hash == payload_hash_of(e.payload)
        assert e.prev_hash == prev
        assert e.hash == chain_hash(prev, e.payload_hash)
        prev = e.hash

    # §3.1 写死的证据顺序：建任务 → 收到事件 → …… → 交付收尾
    kinds = [e.kind for e in events]
    assert kinds[0] is EvidenceKind.task_created
    assert kinds[1] is EvidenceKind.event_received
    assert EvidenceKind.model_call in kinds
    assert EvidenceKind.checklist_op in kinds
    assert EvidenceKind.tool_call in kinds and EvidenceKind.tool_result in kinds
    assert EvidenceKind.artifact in kinds
    assert kinds[-1] is EvidenceKind.delivered

    # Gateway 那条路真的通了：`run_python` 不是被 denied 掉的。
    # 这一条钉的是 TΩ 必须补的那段接线 —— `P0ToolGateway` 的 token 校验是失败关闭的，
    # 没人调 `register_task` / 没传 `token_resolver` 的话，所有 Gateway 工具一律 denied。
    run_python_results = [
        e
        for e in events
        if e.kind is EvidenceKind.tool_result and e.payload["name"] == "run_python"
    ]
    assert len(run_python_results) == 1
    assert run_python_results[0].payload["ok"] is True, (
        f"run_python 没跑成：{run_python_results[0].payload['error']}。"
        "多半是组装没把 Task.session_token 登记给 Gateway"
    )

    manifest = read_manifest(config, task_id)
    assert manifest["root_hash"] == events[-1].hash              # root_hash = 最后一条的 hash
    assert manifest["event_count"] == len(events)
    assert manifest["task_id"] == task_id
    assert manifest["session_id"] == session_id
    assert manifest["created_by"] == "ou_user"
    assert manifest["contract_version"] == CONTRACT_VERSION

    # 库里记的 root_hash 与盘上的 manifest 必须是同一个值，两处口径不许漂
    task = await read_task_from_disk(config, task_id)
    assert task is not None
    assert task.status is TaskStatus.delivered
    assert task.evidence_root_hash == manifest["root_hash"]
    assert manifest["task_no"] == task.task_no
    assert manifest["model"] == task.model

    # 现成的 verify 路径：用一个**全新**的 writer 实例，绕开进程里的 tip 缓存
    assert FileEvidenceWriter(config.storage.evidence_dir).verify(task_id) is True


async def test_tampering_any_line_breaks_verify(config):
    task_id, _ = await _run_one_task(config)
    path = events_path(config, task_id)
    original = path.read_text(encoding="utf-8")
    lines = original.splitlines()
    assert len(lines) >= 8, "这条脚本该产出至少 8 条证据，太少就说明前面哪里没跑到"

    def verify() -> bool:
        # 每次都新建 writer：verify 必须只看文件，不许吃任何进程内缓存
        return FileEvidenceWriter(config.storage.evidence_dir).verify(task_id)

    assert verify() is True

    for i in range(len(lines)):
        record = json.loads(lines[i])
        assert record["payload"] is not None
        record["payload"] = {**record["payload"], "篡改": "有人动过这一行"}
        broken = list(lines)
        broken[i] = json.dumps(record, ensure_ascii=False)
        path.write_text("\n".join(broken) + "\n", encoding="utf-8")
        assert verify() is False, f"第 {i} 行的 payload 被改了，verify 却仍然是 True"

        path.write_text(original, encoding="utf-8")
        assert verify() is True, f"还原第 {i} 行之后 verify 应当回到 True"


async def test_dropping_a_line_breaks_verify(config):
    """整行被删掉（seq 断了 / 链断了）同样要认出来。"""
    task_id, _ = await _run_one_task(config)
    path = events_path(config, task_id)
    lines = path.read_text(encoding="utf-8").splitlines()

    middle = len(lines) // 2
    path.write_text("\n".join(lines[:middle] + lines[middle + 1 :]) + "\n", encoding="utf-8")
    assert FileEvidenceWriter(config.storage.evidence_dir).verify(task_id) is False


#: 停在第 1 次 chat 上不出牌：让任务 A 一直占着那个单 worker，
#: 后面排队的任务 B 就确定性地停在队列里，谁也没在跑它。
HOLD_FOREVER = [final_step("永远到不了这一句。", hold_ticks=10**9)]


async def test_stopped_task_that_never_ran_still_gets_a_manifest(config):
    """`!stop` 掉一个**还没被 worker 领走**的任务，证据链照样要收口。

    这条路不过 `AgentWorker._finish()` —— 任务不在 worker 手里，收尾全在
    `InProcessControlPlane.cancel_task()` 的「不在跑」分支。少了 finalize 的话盘上没有
    manifest.json、库里 `Task.evidence_root_hash` 留空，§2.4 的 `evidence_show.py`
    和卡片上的证据按钮就都指了个空。

    P0 是单 worker：任务 A 停在模型里不出牌，任务 B 就一直排在队列上。
    """
    platform = GatedPlatform()
    model = RecordingModel(HOLD_FOREVER)
    app = build_app(config, platform=platform, model=model, sandbox=FakeSandbox())
    async with running_app(app, shutdown_grace_sec=0.05, exit_timeout=5.0):
        await platform.emit(make_event(event_id="e1", text="干个收不完的活"))
        await wait_until(lambda: model.holds >= 1, what="任务 A 被 worker 领走、停在模型里")
        task_a = (await app.store.list_active_tasks(CHAT))[0]

        # 任务 B：换一条消息 root，于是是新会话新任务，而不是给 A 的 steer
        await platform.emit(make_event(event_id="e2", text="再干一件", message_id="om_2"))
        await wait_until(lambda: app.plane.pending == 1, what="任务 B 排进队列")
        pending = [t for t in await app.store.list_active_tasks(CHAT) if t.id != task_a.id]
        assert len(pending) == 1, f"应当只有任务 B 在等派发，实际 {len(pending)} 个"
        task_b = pending[0]

        await platform.emit(
            make_event(event_id="e3", text=f"!stop {task_b.task_no}", message_id="om_3")
        )
        await wait_until(
            lambda: manifest_path(config, task_b.id).is_file(), what="任务 B 的 manifest 落盘"
        )
        stopped = await app.store.get_task(task_b.id)

    assert stopped is not None
    assert stopped.status is TaskStatus.cancelled

    # 证据链最后一条就是 cancelled，manifest 的 root_hash 与它对得上
    events = read_events(config, task_b.id)
    assert events[-1].kind is EvidenceKind.cancelled
    manifest = read_manifest(config, task_b.id)
    assert manifest["root_hash"] == events[-1].hash
    assert manifest["event_count"] == len(events)
    assert manifest["task_id"] == task_b.id
    assert manifest["session_id"] == task_b.session_id
    assert manifest["task_no"] == task_b.task_no
    assert manifest["created_by"] == "ou_user"
    assert manifest["contract_version"] == CONTRACT_VERSION

    # root_hash 得真的进库 —— cancel_task 里那次 update_task 在 finalize 之前
    from_disk = await read_task_from_disk(config, task_b.id)
    assert from_disk is not None
    assert from_disk.evidence_root_hash == manifest["root_hash"]

    # manifest 的 model 与库里那份同源（worker 收尾的那条路也是这个口径）
    assert manifest["model"] == from_disk.model != ""

    assert FileEvidenceWriter(config.storage.evidence_dir).verify(task_b.id) is True


async def test_evidence_stays_inside_tmp_path(config):
    """整轨的硬约束：一个字节都不许写进仓库的 data/。"""
    task_id, _ = await _run_one_task(config)
    root = str(config.storage.evidence_dir)
    assert str(events_path(config, task_id)).startswith(root)
    assert str(manifest_path(config, task_id)).startswith(root)
    assert "/data/evidence" not in root
