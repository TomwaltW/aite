"""第 1 组：SQLite 真跨进程（§2.2 B6 的进程级版本）。

B6 只验到「同进程里换一个 `ControlPlane` 实例」。这里把标准抬到 §2.4 M6 的口径：
**整套组装拆掉重建**（`build_app` 再走一遍、平台/模型/沙箱全是新的替身、
`store.close()` 真的关过连接），再看同一个话题接不接得上。

判据不止「库里有那一行」：第二套组装的模型上下文里必须真的读到上一轮的正文 ——
接不上上下文的话，续接对用户就是没发生。
"""
import pytest
from app_under_test import build_app
from integration_fakes import (
    CHAT,
    GatedPlatform,
    RecordingModel,
    evidence_task_dirs,
    final_step,
    make_event,
    read_task_from_disk,
    running_app,
    settle,
    wait_until,
)

from aite.contracts import TaskStatus
from aite.testing import FakeSandbox

ROOT = "om_1"


async def test_second_app_on_the_same_db_resumes_the_same_thread(config):
    """一套组装跑完 → 关掉 → 用同一个 .db 新建第二套 → 同话题追问命中同一 session。"""
    # ── 第一套组装 ────────────────────────────────────────────────
    platform1 = GatedPlatform()
    model1 = RecordingModel([final_step("北京今天晴，最高 28℃。")])
    app1 = build_app(config, platform=platform1, model=model1, sandbox=FakeSandbox())

    async with running_app(app1):
        await platform1.emit(make_event(event_id="e1", text="第一问", message_id=ROOT))
        task1 = (await app1.store.list_active_tasks(CHAT))[0]
        session_id = task1.session_id
        await wait_until(lambda: platform1.count("send_text") == 1, what="第一个任务交付")

    # `run_app` 的收尾把连接关了：再用它查任何东西都该报「未初始化」。
    with pytest.raises(RuntimeError):
        await app1.store.get_task(task1.id)

    # ── 第二套组装：同一个 .db 文件，其余全新 ──────────────────────
    platform2 = GatedPlatform()
    model2 = RecordingModel([final_step("好的，按季度再画一张。")])
    app2 = build_app(config, platform=platform2, model=model2, sandbox=FakeSandbox())
    assert app2.store is not app1.store          # 真的是新建的一套，不是复用

    async with running_app(app2):
        # R6：话题内续接，不带 @
        await platform2.emit(
            make_event(
                event_id="e2", text="第二问", mentioned=False, message_id="om_2", thread_id=ROOT
            )
        )
        task2 = (await app2.store.list_active_tasks(CHAT))[0]
        await wait_until(lambda: platform2.count("send_text") == 1, what="第二个任务交付")

        assert task2.session_id == session_id                    # 命中同一会话
        assert (task1.task_no, task2.task_no) == ("#A1", "#A2")  # 任务号计数器也跨进程连续

        turns = await app2.store.list_turns(session_id)
        assert [t.content for t in turns] == ["第一问", "第二问"]
        assert [t.seq for t in turns] == [0, 1]

        # 关键的一条：第二个进程的模型上下文里真的带上了上一轮，
        # 不只是库里躺着一行 turn。
        assert any("第一问" in text for text in model2.prompt_texts())

    # 两个任务，两份证据，都在 tmp_path 下
    assert evidence_task_dirs(config) == sorted([task1.id, task2.id])
    for task_id in (task1.id, task2.id):
        reread = await read_task_from_disk(config, task_id)
        assert reread is not None and reread.status is TaskStatus.delivered


async def test_seen_event_dedupe_survives_the_rebuild(config):
    """R2 的去重键落在库里：换一套组装后重推同一 event_id，不许再跑一遍。

    平台重连后重推是 §2.4 M2 的真实场景，只是这里把「重连」换成了更狠的「重启进程」。
    """
    duplicate = make_event(event_id="ev-dup", text="干个活", message_id=ROOT)

    platform1 = GatedPlatform()
    app1 = build_app(
        config,
        platform=platform1,
        model=RecordingModel([final_step("干完了。")]),
        sandbox=FakeSandbox(),
    )
    async with running_app(app1):
        await platform1.emit(duplicate)
        await wait_until(lambda: platform1.count("send_text") == 1, what="第一次真的跑了")

    platform2 = GatedPlatform()
    model2 = RecordingModel([final_step("不该跑到这一步。")])
    app2 = build_app(config, platform=platform2, model=model2, sandbox=FakeSandbox())
    async with running_app(app2):
        await platform2.emit(duplicate)
        # handle_event 是在 emit 里同步走完的，计数当场就该到位
        assert app2.plane.counters["events.duplicate"] == 1

        await settle()                        # 给派发循环足够的机会去做不该做的事
        assert model2.call_count == 0         # 模型一次都没被叫
        assert platform2.outbound_count == 0  # 一条出站都没有

    assert len(evidence_task_dirs(config)) == 1   # 全程只有一个任务留下证据
