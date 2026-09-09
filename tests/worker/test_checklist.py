"""B3：checklist 协议与卡片。

判据原文：脚本化模型先 `checklist_add` 3 项、逐项 `checklist_check`、最后 `final`
→ FakePlatform 记录到 `send_card`×1、`update_card`≥3、`send_text`×1，
**且没有第二条 `send_card`**。
"""
from worker_fakes import final_turn, tool_turn

from aite.contracts import TaskStatus

SCRIPT = [
    tool_turn(("checklist_add", {"items": ["读取数据", "按月聚合", "画趋势图"]})),
    tool_turn(("checklist_check", {"id": "c1"})),
    tool_turn(("checklist_check", {"id": "c2"})),
    tool_turn(("checklist_check", {"id": "c3"})),
    final_turn("图已画好，趋势见附件。"),
]


async def test_b3_card_lifecycle(run_task, platform):
    # 每步之间推进 0.6s，越过 W4 的 500ms 合并窗口，让每次变更都真的推一次
    _, task, model = await run_task(SCRIPT, step_seconds=0.6)

    assert len(platform.cards) == 1                  # send_card × 1，且没有第二条
    assert len(platform.card_updates) >= 3           # update_card ≥ 3
    assert len(platform.texts) == 1                  # send_text × 1
    assert len(model.calls) == 5
    assert task.status is TaskStatus.delivered


async def test_b3_card_content(run_task, platform, store):
    _, task, _ = await run_task(SCRIPT, step_seconds=0.6)

    first = platform.cards[0][2]
    assert first.status == "working"
    assert first.task_no == task.task_no
    assert first.initiator == "张三"

    last = platform.card_updates[-1][1]
    assert last.status == "delivered"
    assert [i.text for i in last.items] == ["读取数据", "按月聚合", "画趋势图"]
    assert [i.state for i in last.items] == ["done", "done", "done"]
    # 所有更新都打在同一张卡片上（原地更新，绝不新发消息）
    assert {cid for cid, _ in platform.card_updates} == {task.card_id}


async def test_b3_card_is_replied_into_thread_root(run_task, platform, store):
    _, task, _ = await run_task(SCRIPT, step_seconds=0.6)
    session = await store.get_session(task.session_id)

    chat_id, reply_to, _card = platform.cards[0]
    assert chat_id == "oc_chat"
    assert reply_to == session.anchor.thread_id == "om_1"
    assert platform.texts[0].reply_to == "om_1" and platform.texts[0].in_thread is True


# ---- W4：500ms 合并 --------------------------------------------------------

RAPID = [
    tool_turn(("checklist_add", {"items": ["一", "二"]})),
    tool_turn(("checklist_note", {"text": "在算了"})),
    tool_turn(("checklist_note", {"text": "还在算"})),
    tool_turn(("checklist_check", {"id": "c1"})),
    final_turn("好了"),
]


async def test_w4_changes_within_window_collapse(run_task, platform):
    """钟不动 = 所有变更都落在同一个 500ms 窗口 → 只有任务结束那一次 update_card。"""
    _, task, _ = await run_task(RAPID, step_seconds=0.0)

    assert len(platform.cards) == 1
    assert len(platform.card_updates) == 1                 # 4 次变更合并成 1 次
    assert platform.card_updates[0][1].status == "delivered"


async def test_w4_changes_across_windows_are_pushed(run_task, platform):
    """同一个脚本，每步跨过 500ms → 逐次推送。对照上一条。"""
    _, task, _ = await run_task(RAPID, step_seconds=0.6)

    assert len(platform.cards) == 1
    assert len(platform.card_updates) == 4


async def test_checklist_note_lands_on_the_card_not_a_new_message(run_task, platform):
    _, _task, _ = await run_task(RAPID, step_seconds=0.6)

    assert len(platform.texts) == 1                        # 只有 final 那一条
    assert "还在算" in platform.card_updates[-1][1].footer


# ---- checklist 的边角 ------------------------------------------------------


async def test_checklist_items_are_clipped_to_20_chars(run_task, platform):
    long = "这是一条特别特别特别特别特别特别啰嗦的待办项"
    _, _task, _ = await run_task(
        [tool_turn(("checklist_add", {"items": [long]})), final_turn("完")], step_seconds=0.6
    )
    item = platform.card_updates[-1][1].items[0]
    assert len(item.text) <= 20 and item.text.endswith("…")


async def test_checklist_fail_marks_item(run_task, platform):
    script = [
        tool_turn(("checklist_add", {"items": ["下载附件", "解析"]})),
        tool_turn(("checklist_fail", {"id": "c1", "reason": "文件下载失败"})),
        final_turn("附件没拿到，先给了口径说明。"),
    ]
    _, _task, _ = await run_task(script, step_seconds=0.6)

    items = platform.card_updates[-1][1].items
    assert items[0].state == "failed" and items[0].note == "文件下载失败"
    assert items[1].state == "todo"


async def test_bad_checklist_id_is_invalid_args_not_a_crash(run_task, platform):
    script = [
        tool_turn(("checklist_check", {"id": "c9"})),
        final_turn("没有这项，改口径答复。"),
    ]
    _, task, model = await run_task(script, step_seconds=0.6)

    assert task.status is TaskStatus.delivered
    # 模型看到的是一条 role=tool 的错误说明，而不是异常
    tool_msgs = [m for m in model.calls[-1] if m.role == "tool"]
    assert tool_msgs and "没有这一项" in tool_msgs[-1].content
