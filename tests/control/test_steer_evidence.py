"""R6 追问在证据链上留下的那一条（owner: T24）。

T14 把「steer 不写 evidence」钉成了现状并留给总管；这一轨把它补上。补的位置是
`_continue_session` 排 steer 的那一刻，写的是现成的 `event_received`
（`EvidenceKind` 是冻结契约，不新增）。

于是 `event_received` 在时间线上承载两种语义：

    route=new_task   这条事件是任务的起点（R6 新建 / R7 @）
    route=steer      任务已经在跑，用户中途改了要求

两种必须在 payload 上分得开 —— 分不开的话 `scripts/evidence_show.py` 讲出来的故事
是糊的：读的人看到两条一模一样的 `event_received`，不知道哪条是起点、哪条是转折。

这一份验的是「写了什么、什么时候写、什么时候不写」。写完之后**模型真的看见了**
那句话，那是 `tests/worker/test_steer.py` 的地盘（它验的是注入，这里验的是记账）。
"""
import json

from control_fakes import ScriptedModel, make_event

from aite.contracts import EvidenceKind, TaskStatus
from aite.control.plane import ROUTE_NEW_TASK, ROUTE_STEER

CHAT = "oc_chat"
ROOT = "om_1"
FOLLOWUP = "别按月了，改成按季度"


def _events(evidence, task_id: str) -> list[dict]:
    """把这个任务的证据链读成一串 dict（payload 是内联的）。"""
    lines = evidence.events_path(task_id).read_text(encoding="utf-8").splitlines()
    return [json.loads(line) for line in lines]


def _received(evidence, task_id: str) -> list[dict]:
    return [
        e["payload"]
        for e in _events(evidence, task_id)
        if e["kind"] == EvidenceKind.event_received.value
    ]


async def _followup(plane, *, text: str = FOLLOWUP, event_id: str = "ev2", message_id: str = "om_2"):
    await plane.handle_event(
        make_event(
            event_id=event_id, text=text, mentioned=False, message_id=message_id, thread_id=ROOT
        )
    )


# --------------------------------------------------------------------------
# 1 写在哪一步，写了什么
# --------------------------------------------------------------------------

async def test_a_steer_appends_an_event_received_to_the_running_task(make_plane, store, evidence):
    """排 steer 的那一刻就写，不等任务收尾。

    等收尾才写就没意义了：时间线上这条要落在「用户说话的那一刻」，夹在两次
    `model_call` 中间，读的人才看得出「哦，是这里改的主意」。
    """
    plane = make_plane(ScriptedModel([]))          # 脚本空的：任务只入队，不真跑
    await plane.handle_event(make_event(text="按月画个图", message_id=ROOT))
    task = (await store.list_active_tasks(CHAT))[0]

    before = len(_events(evidence, task.id))
    await _followup(plane)

    assert plane.counters["events.steer"] == 1
    assert plane.pending_steer(task.id) == [FOLLOWUP]
    assert len(_events(evidence, task.id)) == before + 1, "排 steer 的当下就该落一条证据"

    payload = _received(evidence, task.id)[-1]
    assert payload == {
        "event_id": "ev2",
        "kind": "message",
        "chat_id": CHAT,
        "sender_id": "ou_user",
        "message_id": "om_2",
        "mentioned": False,
        "route": ROUTE_STEER,
        "text": FOLLOWUP,
    }


async def test_the_two_kinds_of_event_received_are_told_apart_by_route(
    make_plane, store, evidence
):
    """同一条链上两条 `event_received`，`route` 把它们分开。

    分不开的后果很具体：`evidence_show` 的时间线上会出现两条只有 `msg=` 和 `event=`
    不同的行，而那两个 id 对人是没有语义的 —— 讲不出「哪条是任务的起点」。
    """
    plane = make_plane(ScriptedModel([]))
    await plane.handle_event(make_event(text="按月画个图", message_id=ROOT))
    task = (await store.list_active_tasks(CHAT))[0]
    await _followup(plane)

    routes = [p["route"] for p in _received(evidence, task.id)]
    assert routes == [ROUTE_NEW_TASK, ROUTE_STEER], (
        f"起点那条该是 {ROUTE_NEW_TASK}、追问那条该是 {ROUTE_STEER}，实际 {routes}"
    )

    # 顺序也要对：起点在最前面（紧跟 task_created），追问在它之后
    kinds = [e["kind"] for e in _events(evidence, task.id)]
    assert kinds[0] == EvidenceKind.task_created.value
    assert kinds[1] == EvidenceKind.event_received.value
    assert kinds[-1] == EvidenceKind.event_received.value


async def test_the_users_own_words_are_in_the_chain_but_clipped(make_plane, store, evidence):
    """用户说的话进证据，按卡片标题那个截断口径（≤40 字）。

    口径不是这里发明的：`task_created.title` 一直就是把用户原话截断后落盘的。
    W8 后半句禁的是**模型**消息全文，不是用户那句 —— 而正是用户这句话解释了
    「为什么最后交的是季度图而不是月度图」。

    截断的代价写在这里：超过 40 字的追问，证据里只留个开头，全文在 transcript。
    """
    plane = make_plane(ScriptedModel([]))
    await plane.handle_event(make_event(text="按月画个图", message_id=ROOT))
    task = (await store.list_active_tasks(CHAT))[0]

    long_text = "改成按季度，另外" + "把去年同期也画上，" * 20
    await _followup(plane, text=long_text)

    payload = _received(evidence, task.id)[-1]
    assert len(payload["text"]) == 40, f"该截到 40 字，实际 {len(payload['text'])}"
    assert payload["text"].startswith("改成按季度，另外")
    assert payload["text"].endswith("…")

    # 全文没丢，在 transcript 里 —— 证据是索引，不是备份
    turns = await store.list_turns(task.session_id)
    assert turns[-1].content == long_text


async def test_several_steers_each_get_their_own_line(make_plane, store, evidence):
    """连着追问三句，链上就该有三条 —— 合并成一条会把「改了几次主意」抹掉。"""
    plane = make_plane(ScriptedModel([]))
    await plane.handle_event(make_event(text="按月画个图", message_id=ROOT))
    task = (await store.list_active_tasks(CHAT))[0]

    for i in range(3):
        await _followup(plane, text=f"第 {i} 次改主意", event_id=f"ev-{i}", message_id=f"om_{i}")

    steers = [p for p in _received(evidence, task.id) if p["route"] == ROUTE_STEER]
    assert [p["text"] for p in steers] == ["第 0 次改主意", "第 1 次改主意", "第 2 次改主意"]
    assert plane.pending_steer(task.id) == [
        "第 0 次改主意", "第 1 次改主意", "第 2 次改主意"
    ]


# --------------------------------------------------------------------------
# 2 什么时候**不**写
# --------------------------------------------------------------------------

async def test_a_followup_that_becomes_a_new_task_is_not_logged_as_a_steer(
    make_plane, store, evidence
):
    """上一个任务已经交付之后的追问走的是 R6 的「否则新建 task 继续」——
    那是一个新任务的起点，`route` 该是 `new_task`，不是 `steer`。

    记反了的话时间线会讲成「有个任务跑到一半被改了要求」，而实际上是两次独立的对话。
    """
    plane = make_plane(ScriptedModel([]))
    await plane.handle_event(make_event(text="按月画个图", message_id=ROOT))
    first = (await store.list_active_tasks(CHAT))[0]
    first.status = TaskStatus.delivered
    await store.update_task(first)

    await _followup(plane)

    fresh = [t for t in await store.list_active_tasks(CHAT) if t.id != first.id]
    assert len(fresh) == 1
    assert plane.counters["events.steer"] == 0

    # 老任务的链没被动过；新任务自己有一条 new_task
    assert [p["route"] for p in _received(evidence, first.id)] == [ROUTE_NEW_TASK]
    assert [p["route"] for p in _received(evidence, fresh[0].id)] == [ROUTE_NEW_TASK]


async def test_an_orphan_task_gets_no_steer_evidence(make_plane, store, evidence, config):
    """孤儿任务（上个进程留下的 working）不收追问，也就不该往它的链上写东西。

    T14 修的正是这条：孤儿不算 steer 目标，追问走「新建 task」。写证据这一步挂在
    `target is not None` 里面，所以它天然跟着那个判断走 —— 这条用例把「天然」钉住，
    免得哪天有人把 evidence 写到 `if` 外面去，给一个永远没人 drain 的队列记账。
    """
    plane = make_plane(ScriptedModel([]))
    await plane.handle_event(make_event(text="按月画个图", message_id=ROOT))
    orphan = (await store.list_active_tasks(CHAT))[0]
    orphan.status = TaskStatus.working
    await store.update_task(orphan)
    plane._owned.discard(orphan.id)                # 换进程 = 本进程没接手过它

    await _followup(plane)

    assert plane.counters["events.orphan_task"] == 1
    assert plane.counters["events.steer"] == 0
    assert [p["route"] for p in _received(evidence, orphan.id)] == [ROUTE_NEW_TASK], (
        "孤儿的链上不该多出一条它永远收不到的追问"
    )


async def test_a_command_in_the_thread_is_not_a_steer(make_plane, store, evidence):
    """`!status` 这类命令在 R5 就被截走了，走不到 R6，链上不该有它。

    证据链讲的是「这个任务经历了什么」，不是「这个群里发生过什么」。
    """
    plane = make_plane(ScriptedModel([]))
    await plane.handle_event(make_event(text="按月画个图", message_id=ROOT))
    task = (await store.list_active_tasks(CHAT))[0]

    await _followup(plane, text="!status", event_id="ev-cmd", message_id="om_cmd")

    assert plane.counters["events.steer"] == 0
    assert [p["route"] for p in _received(evidence, task.id)] == [ROUTE_NEW_TASK]


# --------------------------------------------------------------------------
# 3 链本身还得是条链
# --------------------------------------------------------------------------

async def test_the_chain_still_verifies_after_a_steer(make_plane, store, evidence):
    """多写一条不能把 hash 链写断 —— 证据链断了，整份证据就不可信了（C1/§2.4）。"""
    plane = make_plane(ScriptedModel([]))
    await plane.handle_event(make_event(text="按月画个图", message_id=ROOT))
    task = (await store.list_active_tasks(CHAT))[0]
    await _followup(plane)
    await _followup(plane, text="再加上同比", event_id="ev3", message_id="om_3")

    events = _events(evidence, task.id)
    assert [e["seq"] for e in events] == list(range(len(events))), "seq 该是连着的"
    assert evidence.verify(task.id) is True, "加了 steer 之后链校验不过了"
