"""T2 独有验收：`!status` / `!stop` / `!restart` / `!new` 四条命令（§3.5 R5 + §3.3）。"""
from control_fakes import make_event

from aite.contracts import SessionStatus, TaskStatus
from aite.control import UNKNOWN_COMMAND_TEXT

CHAT = "oc_chat"
ROOT = "om_1"


async def _cmd(plane, text, *, message_id="om_9", thread_id=None, mentioned=True, event_id=None):
    await plane.handle_event(
        make_event(
            event_id=event_id or f"ev-{message_id}",
            text=text,
            mentioned=mentioned,
            message_id=message_id,
            thread_id=thread_id,
        )
    )


# ---- !status --------------------------------------------------------------


async def test_status_empty(plane, platform):
    await _cmd(plane, "!status")
    assert platform.texts[-1].text == "本群没有活跃任务"


async def test_status_lists_active_tasks(plane, store, platform):
    await plane.handle_event(make_event(event_id="e1", text="画个趋势图", message_id=ROOT))
    task = (await store.list_active_tasks(CHAT))[0]

    await _cmd(plane, "!status")

    body = platform.texts[-1].text
    assert task.task_no in body and "画个趋势图" in body and "created" in body


async def test_status_is_scoped_to_this_chat(plane, store, platform):
    await plane.handle_event(make_event(event_id="e1", text="别的群的活", message_id="om_x", chat_id="oc_other"))
    await _cmd(plane, "!status")
    assert platform.texts[-1].text == "本群没有活跃任务"


# ---- !stop ----------------------------------------------------------------


async def test_stop_cancels_and_releases_sandbox(plane, store, platform, sandbox):
    await plane.handle_event(make_event(event_id="e1", text="跑个长活", message_id=ROOT))
    task = (await store.list_active_tasks(CHAT))[0]
    task.card_id, task.sandbox_id = "om_card_1", "sb_x"
    await store.update_task(task)

    await _cmd(plane, f"!stop {task.task_no}")

    assert (await store.get_task(task.id)).status is TaskStatus.cancelled
    assert sandbox.released == ["sb_x"]
    assert platform.card_updates[-1][0] == "om_card_1"
    assert platform.card_updates[-1][1].status == "cancelled"
    assert task.task_no in platform.texts[-1].text


async def test_stop_accepts_task_no_without_hash(plane, store):
    await plane.handle_event(make_event(event_id="e1", text="活儿", message_id=ROOT))
    task = (await store.list_active_tasks(CHAT))[0]

    await _cmd(plane, f"!stop {task.task_no.lstrip('#').lower()}")

    assert (await store.get_task(task.id)).status is TaskStatus.cancelled


async def test_stop_unknown_task(plane, platform):
    await _cmd(plane, "!stop #A99")
    assert platform.texts[-1].text == "没有这个任务"


# ---- !restart -------------------------------------------------------------


async def test_restart_archives_session_and_starts_new_one(plane, store, platform):
    await plane.handle_event(make_event(event_id="e1", text="第一版方案", message_id=ROOT))
    old = await store.find_session_by_thread(CHAT, ROOT)

    await _cmd(plane, "!restart 换个思路重来", message_id="om_2", thread_id=ROOT, mentioned=False)

    assert (await store.get_session(old.id)).status is SessionStatus.archived
    new = await store.find_session_by_thread(CHAT, ROOT)
    assert new is not None and new.id != old.id        # 同一话题，换了会话
    assert new.anchor.thread_id == ROOT

    tasks = await store.list_active_tasks(CHAT)
    assert len(tasks) == 1 and tasks[0].session_id == new.id   # 旧会话那个已被终止
    assert tasks[0].title == "换个思路重来"
    assert [t.content for t in await store.list_turns(new.id)] == ["换个思路重来"]
    assert "终止了 1 个进行中的任务" in platform.texts[-1].text


async def test_restart_without_text_only_archives(plane, store, platform):
    await plane.handle_event(make_event(event_id="e1", text="第一版", message_id=ROOT))
    old = await store.find_session_by_thread(CHAT, ROOT)

    await _cmd(plane, "!restart", message_id="om_2", thread_id=ROOT, mentioned=False)

    assert (await store.get_session(old.id)).status is SessionStatus.archived
    assert await store.list_active_tasks(CHAT) == []
    assert "请直接说要做什么" in platform.texts[-1].text


# ---- !new -----------------------------------------------------------------


async def test_new_forces_fresh_session_inside_existing_thread(plane, store):
    await plane.handle_event(make_event(event_id="e1", text="老话题", message_id=ROOT))
    old = await store.find_session_by_thread(CHAT, ROOT)

    await _cmd(plane, "!new 另起一件事", message_id="om_5", thread_id=ROOT, mentioned=False)

    assert (await store.find_session_by_thread(CHAT, ROOT)).id == old.id   # 老话题没被动
    fresh = await store.find_session_by_thread(CHAT, "om_5")
    assert fresh is not None and fresh.id != old.id                        # 本条消息成了新 root

    tasks = await store.list_active_tasks(CHAT)
    assert {t.session_id for t in tasks} == {old.id, fresh.id}
    assert next(t for t in tasks if t.session_id == fresh.id).title == "另起一件事"


# ---- 未知命令 -------------------------------------------------------------


async def test_unknown_command(plane, platform):
    await _cmd(plane, "!oops 干点啥")
    assert platform.texts[-1].text == UNKNOWN_COMMAND_TEXT
    assert "!status" in UNKNOWN_COMMAND_TEXT and "!restart" in UNKNOWN_COMMAND_TEXT
