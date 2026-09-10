"""R6 排队的 steer 消息，worker 每步开始前合并进上下文。

主干那条（第 1 步跑起来后往话题里塞一句不带 @ 的话 → 下一步进上下文）在第一个用例。
其余是读实现时看得见、但没人验过的合并侧边界：

- 连发几句的顺序、是几条消息还是并成一条；
- 在 W1 那五层里插在哪 —— 插错了模型会把它当历史而不是新指令；
- 任务还在队列里排着的时候排进来的 steer，会不会和 transcript 里的同一句话撞车；
- `!stop` 抢在 drain 前面时，这句话到底有没有进模型；
- 证据链上看不看得见「任务跑到一半用户改了要求」；
- 一句几千字的追问有没有被截断。

排队侧（排给谁、任务没跑起来时队列谁清）在 tests/control/test_steer_routing.py。
"""
from worker_fakes import (
    FakePlatform,
    ScriptedModel,
    final_turn,
    history,
    make_event,
    tool_turn,
)

from aite.contracts import Attachment
from aite.control import InProcessControlPlane
from aite.worker.context import ATTACHMENT_HEADER, HISTORY_HEADER

STEER = "顺便加上同比"
ROOT = "om_1"


def _plane(*, store, config, clock, sandbox, gateway, evidence, model, platform) -> InProcessControlPlane:
    return InProcessControlPlane(
        store=store,
        platform=platform,
        evidence=evidence,
        config=config,
        model=model,
        gateway=gateway,
        sandbox=sandbox,
        clock=clock,
        sleep=clock.sleep,
    )


def _followup(text: str = STEER, *, event_id: str = "ev2", message_id: str = "om_2"):
    """话题里不带 @ 的一句话 —— R6 判定为 steer。"""
    return make_event(
        event_id=event_id, text=text, mentioned=False, message_id=message_id, thread_id=ROOT
    )


def _inject_at(box: list, step: int, *events):
    """在第 `step` 步开始前把这些事件投进控制面（ScriptedModel.on_call 的钩子）。"""

    async def inject(current: int) -> None:
        if current == step:
            for ev in events:
                await box[0].handle_event(ev)

    return inject


async def test_steer_message_reaches_the_next_step(store, config, clock, sandbox, gateway, evidence):
    platform = FakePlatform()
    box: list = []

    model = ScriptedModel(
        [
            tool_turn(("checklist_add", {"items": ["取数"]})),
            tool_turn(("checklist_check", {"id": "c1"})),
            final_turn("加上同比之后的结果。"),
        ],
        clock=clock,
        step_seconds=0.6,
        on_call=_inject_at(box, 1, _followup()),
    )
    plane = _plane(
        store=store, config=config, clock=clock, sandbox=sandbox, gateway=gateway,
        evidence=evidence, model=model, platform=platform,
    )
    box.append(plane)

    await plane.handle_event(make_event(text="按月画个图"))
    task = (await store.list_active_tasks("oc_chat"))[0]
    await plane.run_pending()

    # 第 2 步（下标 1）之前注入，所以第 3 步（下标 2）的上下文里必须有它
    step3 = model.calls[2]
    assert any(m.role == "user" and m.content == STEER for m in step3)
    assert not any(m.role == "user" and m.content == STEER for m in model.calls[1])

    assert plane.pending_steer(task.id) == []            # 已被消费
    assert len(await store.list_active_tasks("oc_chat")) == 0   # 没有第二个 task
    assert [t.content for t in await store.list_turns(task.session_id)] == ["按月画个图", STEER]


async def test_several_steer_messages_keep_their_order_as_separate_turns(
    store, config, clock, sandbox, gateway, evidence
):
    """连发三句：`drain_steer()` 原序出来，合并成三条独立的 user 消息而不是并成一条。

    并成一条的话模型看到的是一段拼接文本，分不出这是三次追问；顺序反了更糟 ——
    「不要月度」「改成季度」倒过来读意思是反的。
    """
    texts = ["顺便加上同比", "不要月度了", "改成季度"]
    box: list = []
    model = ScriptedModel(
        [
            tool_turn(("checklist_add", {"items": ["取数"]})),
            tool_turn(("checklist_check", {"id": "c1"})),
            final_turn("按季度算好了。"),
        ],
        clock=clock,
        on_call=_inject_at(
            box,
            1,
            *[
                _followup(t, event_id=f"ev{i + 2}", message_id=f"om_{i + 2}")
                for i, t in enumerate(texts)
            ],
        ),
    )
    plane = _plane(
        store=store, config=config, clock=clock, sandbox=sandbox, gateway=gateway,
        evidence=evidence, model=model, platform=FakePlatform(),
    )
    box.append(plane)

    await plane.handle_event(make_event(text="按月画个图"))
    task = (await store.list_active_tasks("oc_chat"))[0]
    await plane.run_pending()

    step3 = model.calls[2]
    assert [m.content for m in step3[-3:]] == texts       # 原序，且是尾巴上连着的三条
    assert [m.role for m in step3[-3:]] == ["user"] * 3
    assert plane.pending_steer(task.id) == []
    assert [t.content for t in await store.list_turns(task.session_id)] == ["按月画个图", *texts]


async def test_steer_lands_at_the_tail_after_the_history_and_attachment_blocks(
    store, config, clock, sandbox, gateway, evidence
):
    """W1 的顺序是 system → transcript → 群历史 → 附件清单 → 工具目录。

    steer 是**运行中新到的指令**，不是历史的一部分，所以只能落在这五层之后、
    上一步的 assistant/tool 消息之后 —— 也就是消息列表的最末尾。插进 transcript
    那一层的话，模型读到的就是「用户当时说过这么一句」而不是「现在要求改了」。
    """
    platform = FakePlatform(history=history(("om_h1", "human", "李四", "这周的数在共享盘")))
    box: list = []
    model = ScriptedModel(
        [
            tool_turn(("checklist_add", {"items": ["取数"]})),
            tool_turn(("checklist_check", {"id": "c1"})),
            final_turn("好了。"),
        ],
        clock=clock,
        on_call=_inject_at(box, 1, _followup()),
    )
    plane = _plane(
        store=store, config=config, clock=clock, sandbox=sandbox, gateway=gateway,
        evidence=evidence, model=model, platform=platform,
    )
    box.append(plane)

    await plane.handle_event(
        make_event(
            text="按月画个图",
            attachments=[
                Attachment(kind="file", file_key="fk_1", message_id=ROOT, name="sales.csv", size=64)
            ],
        )
    )
    await plane.run_pending()

    step3 = model.calls[2]
    h_idx = next(i for i, m in enumerate(step3) if HISTORY_HEADER in (m.content or ""))
    a_idx = next(i for i, m in enumerate(step3) if ATTACHMENT_HEADER in (m.content or ""))
    s_idx = next(i for i, m in enumerate(step3) if m.content == STEER)

    assert h_idx < a_idx < s_idx
    assert s_idx == len(step3) - 1                        # 就在最末尾
    # transcript 那一层还是任务开跑时的样子，没被 steer 挤进去
    assert step3[1].role == "user" and step3[1].content == "按月画个图"


async def test_steer_queued_before_the_task_starts_is_not_injected_twice(
    store, config, clock, sandbox, gateway, evidence
):
    """任务还躺在队列里（status=created）就来的追问：`_continue_session` 先
    `append_turn` 落库、再排队，而 worker 是开跑时才去 `list_turns` ——
    transcript 里本来就有这句话了，队列里那份再合并一次就是同一句话进两遍。

    这条路不是边角：评测 02 的 `after: none` 对照组走的正是它
    （见 tests/e2e/test_t5_event_timing.py），人打字快一点真机上就是这个时序。
    """
    model = ScriptedModel([final_turn("算好了。")], clock=clock)
    plane = _plane(
        store=store, config=config, clock=clock, sandbox=sandbox, gateway=gateway,
        evidence=evidence, model=model, platform=FakePlatform(),
    )

    await plane.handle_event(make_event(text="按月画个图"))
    task = (await store.list_active_tasks("oc_chat"))[0]
    await plane.handle_event(_followup())                 # 任务还没被 worker 领走
    assert plane.pending_steer(task.id) == [STEER]        # 排队侧照 R6 排上了

    await plane.run_pending()

    step1 = model.calls[0]
    assert [m.content for m in step1].count(STEER) == 1, "同一句话不许进两遍"
    assert plane.pending_steer(task.id) == []
    # 进上下文的那一份来自 transcript，位置在 system prompt 之后的第二条
    assert step1[1].content == "按月画个图" and step1[2].content == STEER


async def test_stop_before_the_drain_keeps_the_steer_out_of_the_model(
    store, config, clock, sandbox, gateway, evidence
):
    """steer 排着队时 `!stop` 掉这个任务：主循环的取消判定在 drain 前面，
    所以这句话不会再进模型；队列也要跟着清掉，不许留在内存里。
    """
    box: list = []

    async def inject(step: int) -> None:
        if step != 1:
            return
        await box[0].handle_event(_followup())
        live = (await store.list_active_tasks("oc_chat"))[0]
        await box[0].cancel_task(live)

    model = ScriptedModel(
        [
            tool_turn(("checklist_add", {"items": ["取数"]})),
            tool_turn(("checklist_check", {"id": "c1"})),
            final_turn("不该走到这里。"),
        ],
        clock=clock,
        on_call=inject,
    )
    plane = _plane(
        store=store, config=config, clock=clock, sandbox=sandbox, gateway=gateway,
        evidence=evidence, model=model, platform=FakePlatform(),
    )
    box.append(plane)

    await plane.handle_event(make_event(text="按月画个图"))
    task = (await store.list_active_tasks("oc_chat"))[0]
    await plane.run_pending()

    assert len(model.calls) == 2                          # 第 3 步在取消判定处就返回了
    assert not any(m.content == STEER for call in model.calls for m in call)
    assert plane.pending_steer(task.id) == []
    assert plane._steer == {}
    assert (await store.get_task(task.id)).status.value == "cancelled"


async def test_steer_is_recorded_in_the_evidence_chain(
    store, config, clock, sandbox, gateway, evidence
):
    """T24 关掉了 T14 在这里留的口子（本条原名 `..._leaves_no_trace_...`）。

    T14 钉的现状是「steer 不写 evidence」：W8 只点名 `model_call / tool_call /
    tool_result / checklist_op` 四类，steer 不在其中，于是 evidence_show 的时间线上
    缺了「任务跑到一半用户改了要求」这一段 —— 而 `model_call` 只记 `messages_hash`，
    原文不落盘，从证据里也反推不出来。总管拍板补上。

    补法：`_continue_session` 排 steer 时给目标任务写一条 `event_received`
    （不新增 `EvidenceKind`，那是冻结契约），payload 带 `route: "steer"` 与截断后的
    用户原话。两种 `event_received` 靠 `route` 分开：`new_task` 是任务的起点，
    `steer` 是中途改的要求。
    """
    box: list = []
    model = ScriptedModel(
        [
            tool_turn(("checklist_add", {"items": ["取数"]})),
            tool_turn(("checklist_check", {"id": "c1"})),
            final_turn("好了。"),
        ],
        clock=clock,
        on_call=_inject_at(box, 1, _followup()),
    )
    plane = _plane(
        store=store, config=config, clock=clock, sandbox=sandbox, gateway=gateway,
        evidence=evidence, model=model, platform=FakePlatform(),
    )
    box.append(plane)

    await plane.handle_event(make_event(text="按月画个图"))
    task = (await store.list_active_tasks("oc_chat"))[0]
    await plane.run_pending()

    lines = evidence.events_path(task.id).read_text(encoding="utf-8").splitlines()
    assert lines, "任务跑完了却一条证据都没有"
    assert any('"route":"steer"' in line for line in lines), (
        "追问排进来了，证据链上却找不到 route=steer 那一条"
    )
    assert any(STEER in line for line in lines), (
        "证据里读不到用户把要求改成了什么 —— 那这条时间线还是讲不清最后为什么交了这个"
    )
    # 建任务那条也认得出自己是谁，两种语义在 payload 上分得开
    assert any('"route":"new_task"' in line for line in lines)
    # 模型确实看到了它 —— 证据记的和模型收到的是同一件事
    assert any(m.content == STEER for m in model.calls[2])


async def test_a_very_long_steer_is_not_truncated(
    store, config, clock, sandbox, gateway, evidence
):
    """几千字的追问原样进上下文。

    与 transcript 里的普通 user turn 同口径：W1 只按**轮数**截断（40 轮留头 2 尾 30），
    从来不按字数；契约里也没有用户文本的字数上限（`MAX_EXEC_OUTPUT_CHARS` /
    `MAX_TOOL_CONTENT_CHARS` 管的是工具输出）。只在 steer 这一条路上单独加个字数上限，
    会让「同一句话走 transcript 不截、走 steer 被截」，那才是真的怪。
    """
    long_text = "再确认一遍口径" * 600                     # 4200 字
    box: list = []
    model = ScriptedModel(
        [
            tool_turn(("checklist_add", {"items": ["取数"]})),
            tool_turn(("checklist_check", {"id": "c1"})),
            final_turn("好了。"),
        ],
        clock=clock,
        on_call=_inject_at(box, 1, _followup(long_text)),
    )
    plane = _plane(
        store=store, config=config, clock=clock, sandbox=sandbox, gateway=gateway,
        evidence=evidence, model=model, platform=FakePlatform(),
    )
    box.append(plane)

    await plane.handle_event(make_event(text="按月画个图"))
    await plane.run_pending()

    merged = [m for m in model.calls[2] if m.role == "user" and m.content == long_text]
    assert len(merged) == 1
    assert len(merged[0].content) == len(long_text) == 4200
