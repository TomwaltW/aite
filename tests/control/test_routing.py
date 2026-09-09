"""B2：§3.5 路由规则 R1–R8，每条至少一个用例。

规则按编号顺序求值、命中即停，所以除了「这条规则做对了什么」，
还要盯住「更靠后的规则没有被顺带触发」——每个用例都断言了这一点。
"""
from control_fakes import make_event

from aite.contracts import CardAction, EventKind, SenderKind, TaskStatus


async def _tasks(store, chat_id="oc_chat"):
    return await store.list_active_tasks(chat_id)


# ---- R1：sender_kind != human 一律丢弃 -----------------------------------


async def test_r1_bot_at_never_starts_task(plane, store, platform):
    """机器人 @ 了 Aite 也不建会话、不出站。含 Aite 自己发的消息。"""
    await plane.handle_event(make_event(sender_kind=SenderKind.bot, text="@Aite 帮我算一下"))
    await plane.handle_event(make_event(event_id="ev2", sender_kind=SenderKind.app, message_id="om_2"))
    await plane.handle_event(make_event(event_id="ev3", sender_kind=SenderKind.system, message_id="om_3"))

    assert plane.counters["events.nonhuman"] == 3
    assert await _tasks(store) == []
    assert platform.reactions == [] and platform.texts == []


# ---- R2：seen_event 去重 --------------------------------------------------


async def test_r2_duplicate_event_makes_one_task(plane, store):
    """重连后平台重推同一 event_id → 只建一个 task。"""
    ev = make_event(event_id="ev-dup")
    await plane.handle_event(ev)
    await plane.handle_event(ev)

    assert plane.counters["events.duplicate"] == 1
    assert len(await _tasks(store)) == 1


# ---- R3：card_action ------------------------------------------------------


async def test_r3_card_stop_cancels_task_without_new_session(plane, store, sandbox):
    await plane.handle_event(make_event(text="跑个数"))
    task = (await _tasks(store))[0]
    task.card_id, task.sandbox_id = "om_card_1", "sb_1"
    await store.update_task(task)

    await plane.handle_event(
        make_event(
            event_id="ev-card",
            kind=EventKind.card_action,
            text="",
            message_id="om_9",
            card_action=CardAction(card_id="om_card_1", action="stop", task_id=task.id),
        )
    )

    assert (await store.get_task(task.id)).status is TaskStatus.cancelled
    assert sandbox.released == ["sb_1"]
    assert await _tasks(store) == []                                   # 不再是活跃任务
    assert await store.find_session_by_thread("oc_chat", "om_9") is None  # R3 不建会话


async def test_r3_card_evidence_replies_with_path(plane, store, platform, config):
    await plane.handle_event(make_event(text="跑个数"))
    task = (await _tasks(store))[0]

    await plane.handle_event(
        make_event(
            event_id="ev-card2",
            kind=EventKind.card_action,
            text="",
            message_id="om_9",
            card_action=CardAction(card_id="om_card_1", action="evidence", task_id=task.id),
        )
    )

    assert task.id in platform.texts[-1].text
    assert config.storage.evidence_dir in platform.texts[-1].text


# ---- R4：编辑 / 删除 ------------------------------------------------------


async def test_r4_edit_writes_system_note_and_starts_nothing(plane, store):
    """编辑即使加上 @Aite 也不启动任务。"""
    await plane.handle_event(make_event(text="画个图"))
    session_id = (await _tasks(store))[0].session_id
    before = len(await _tasks(store))

    await plane.handle_event(
        make_event(
            event_id="ev-edit",
            kind=EventKind.message_edited,
            text="@Aite 画个柱状图",
            mentioned=True,
            message_id="om_1",       # 编辑的正是话题 root 那条
        )
    )

    turns = await store.list_turns(session_id)
    assert turns[-1].role == "system_note"
    assert "[用户修改了消息] 新内容：@Aite 画个柱状图" == turns[-1].content
    assert len(await _tasks(store)) == before


async def test_r4_delete_does_nothing(plane, store, platform):
    await plane.handle_event(make_event(text="画个图"))
    turns_before = await store.list_turns((await _tasks(store))[0].session_id)

    await plane.handle_event(
        make_event(event_id="ev-del", kind=EventKind.message_deleted, text="", message_id="om_1")
    )

    assert plane.counters["events.deleted"] == 1
    assert await store.list_turns((await _tasks(store))[0].session_id) == turns_before
    assert len(platform.texts) == 0


# ---- R5：`!` 命令先于 R6/R7 -----------------------------------------------


async def test_r5_command_in_thread_beats_r6(plane, store, platform):
    """话题内的 `!status` 走命令，不当成续接消息去建 task。"""
    await plane.handle_event(make_event(text="第一件事"))
    root = "om_1"
    tasks_before = await _tasks(store)

    await plane.handle_event(
        make_event(event_id="ev-cmd", text="!status", mentioned=False, message_id="om_2", thread_id=root)
    )

    assert await _tasks(store) == tasks_before          # 没有新 task
    assert "本群活跃任务" in platform.texts[-1].text


async def test_r5_command_without_at_or_thread_is_not_a_command(plane, store, platform):
    """既没 @ 也不在话题里的 `!xxx` 不算命令，落到 R8 被丢弃。"""
    await plane.handle_event(
        make_event(event_id="ev-cmd2", text="!status", mentioned=False, message_id="om_5")
    )

    assert platform.texts == []
    assert plane.counters["events.ignored"] == 1


# ---- R6：话题内续接，不要求 mentioned -------------------------------------


async def test_r6_followup_hits_same_session_and_starts_new_task(plane, store, platform):
    await plane.handle_event(make_event(text="第一问"))
    first = (await _tasks(store))[0]
    first.status = TaskStatus.delivered            # 上一个任务已结束
    await store.update_task(first)

    await plane.handle_event(
        make_event(event_id="ev2", text="再按季度画一张", mentioned=False, message_id="om_2", thread_id="om_1")
    )

    active = await _tasks(store)
    assert len(active) == 1
    assert active[0].session_id == first.session_id      # 同一 session
    assert active[0].id != first.id                      # 新 task
    assert active[0].task_no != first.task_no


async def test_r6_followup_with_active_task_queues_steer(plane, store):
    await plane.handle_event(make_event(text="第一问"))
    task = (await _tasks(store))[0]

    await plane.handle_event(
        make_event(event_id="ev2", text="顺便加上同比", mentioned=False, message_id="om_2", thread_id="om_1")
    )

    assert plane.pending_steer(task.id) == ["顺便加上同比"]
    assert len(await _tasks(store)) == 1                 # 没有第二个 task
    assert plane.counters["events.steer"] == 1


# ---- R7：@ 新建 -----------------------------------------------------------


async def test_r7_mention_creates_session_rooted_at_this_message(plane, store, platform):
    await plane.handle_event(make_event(text="帮我看下这个", message_id="om_root"))

    task = (await _tasks(store))[0]
    session = await store.get_session(task.session_id)
    assert session.anchor.thread_id == "om_root"         # 本条消息成为话题 root
    assert platform.reactions == [("om_root", "ack")]
    assert task.task_no.startswith("#A")
    assert plane.pending == 1                            # 已入队


# ---- R8：其余丢弃 ---------------------------------------------------------


async def test_r8_plain_group_message_is_dropped(plane, store, platform):
    await plane.handle_event(
        make_event(event_id="ev-plain", text="大家早", mentioned=False, message_id="om_7")
    )

    assert plane.counters["events.ignored"] == 1
    assert await _tasks(store) == []
    assert platform.texts == [] and platform.reactions == []
