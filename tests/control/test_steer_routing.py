"""R6 排队侧的边界：steer 排给谁、什么时候不排、任务没跑起来时队列谁来清。

`tests/control/test_routing.py` 只验了 R6 的主干（话题内追问 → 排成 steer / 新建
task）。这一份补的是主干外面那几条 —— 都是读实现时看得见、但没人验过的地方：

- 队列只在内存里（`InProcessControlPlane._steer`），进程一换就没了；
- 换进程之后，库里还挂在 `working` 上的旧任务没人会再领，追问排给它 = 扔了；
- 任务压根没被 worker 领走（已取消 / 没配 worker）时，队列谁来清。
"""
from datetime import UTC, datetime, timedelta

from control_fakes import FakeClock, FakePlatform, ScriptedModel, make_config, make_event

from aite.contracts import Task, TaskStatus
from aite.control import InProcessControlPlane, SqliteSessionStore
from aite.evidence import FileEvidenceWriter

CHAT = "oc_chat"
ROOT = "om_1"


def _plane(store, config, model=None) -> InProcessControlPlane:
    """照 test_persistence.py 的写法：同一个 SQLite 文件换一个控制面实例 = 换进程。"""
    clock = FakeClock()
    return InProcessControlPlane(
        store=store,
        platform=FakePlatform(),
        evidence=FileEvidenceWriter(config.storage.evidence_dir),
        config=config,
        model=model,
        clock=clock,
        sleep=clock.sleep,
    )


def _task(task_id: str, session_id: str, *, minutes: int) -> Task:
    """给 `_steer_target` 造候选。`minutes` 决定 created_at 的先后。"""
    now = datetime(2026, 9, 10, 12, 0, tzinfo=UTC) + timedelta(minutes=minutes)
    return Task(
        id=task_id,
        session_id=session_id,
        task_no=f"#A{minutes}",
        session_token="tok",
        created_by="ou_user",
        created_at=now,
        updated_at=now,
    )


async def _steer_once(plane, *, text="顺便加上同比", event_id="ev2", message_id="om_2"):
    await plane.handle_event(
        make_event(
            event_id=event_id, text=text, mentioned=False, message_id=message_id, thread_id=ROOT
        )
    )


# ---- ① 队列只在内存里 -----------------------------------------------------


async def test_pending_steer_does_not_survive_a_restart(tmp_path):
    """**当前边界，不是缺陷判定**：`append_turn` 那一半落库，排队那一半不落。

    §2.4 M6 要的是「杀进程重启后在旧线程追问仍能续接」—— 会话锚点和 transcript 都在
    库里，那一条成立（见 test_persistence.py 的 B6）。丢的是「这句话本该被合并进正在
    跑的那个任务」这件事。要不要把队列也落库涉及改 §3.2 `SessionStore` 协议，
    那是 T0 的地盘，本轨只钉事实。
    """
    config = make_config(tmp_path)
    db = config.storage.sqlite_path

    store1 = SqliteSessionStore(db)
    await store1.init()
    plane1 = _plane(store1, config)
    await plane1.handle_event(make_event(event_id="ev1", text="第一问", message_id=ROOT))
    task = (await store1.list_active_tasks(CHAT))[0]
    await _steer_once(plane1)

    assert plane1.pending_steer(task.id) == ["顺便加上同比"]   # 排在内存里
    await store1.close()

    store2 = SqliteSessionStore(db)
    await store2.init()
    plane2 = _plane(store2, config)

    assert plane2.pending_steer(task.id) == []                 # 换实例 = 队列全丢

    # 落库的那一半还在：用户说过的话在 transcript 里，下一个任务的上下文能读到它
    turns = await store2.list_turns(task.session_id)
    assert [t.content for t in turns] == ["第一问", "顺便加上同比"]
    await store2.close()


async def test_followup_after_a_restart_starts_a_new_task_instead_of_feeding_an_orphan(tmp_path):
    """杀进程时在跑的任务，库里还是 `working`，但本进程谁也不会去领它。

    改动前：R6 看到「本会话有活跃 task」→ 把追问排进 `self._steer[孤儿]`，那条队列
    永远没人 drain，用户看到的就是 Aite 一点反应都没有 —— M6 直接红。
    改动后：孤儿不算 steer 目标，走 R6 原文的「否则新建 task 继续」。
    """
    config = make_config(tmp_path)
    db = config.storage.sqlite_path

    store1 = SqliteSessionStore(db)
    await store1.init()
    plane1 = _plane(store1, config)
    await plane1.handle_event(make_event(event_id="ev1", text="按月画个图", message_id=ROOT))
    orphan = (await store1.list_active_tasks(CHAT))[0]
    orphan.status = TaskStatus.working          # 被杀的那一刻正在跑
    await store1.update_task(orphan)
    await store1.close()

    store2 = SqliteSessionStore(db)
    await store2.init()
    plane2 = _plane(store2, config)
    await _steer_once(plane2, text="再按季度画一张", event_id="ev2")

    active = await store2.list_active_tasks(CHAT)
    fresh = [t for t in active if t.id != orphan.id]
    assert len(fresh) == 1, "追问应当新建一个 task，而不是排给孤儿"
    assert fresh[0].session_id == orphan.session_id             # R6：续接同一会话
    assert plane2.counters["events.orphan_task"] == 1
    assert plane2.counters["events.steer"] == 0
    assert plane2.pending_steer(orphan.id) == []
    assert plane2._steer == {}                                  # 也没留下空条目

    # 残留（报给总管）：孤儿本身还挂在 active 上，`!status` 里看得见，也没人给它收尾。
    # 那要么靠起飞时的对账、要么靠 §3.2 加口子，都超出本轨可写面。
    assert orphan.id in {t.id for t in active}
    await store2.close()


# ---- ② 排给哪个任务 -------------------------------------------------------


async def test_steer_target_prefers_the_running_task_and_ignores_orphans(plane):
    """白盒测 `_steer_target`：P0 的路由不会让同一会话出现两个活跃 task
    （`_continue_session` 有活跃任务就只排 steer、不新建），所以这个分支从事件面
    造不出来 —— 直接喂候选列表。

    要钉住三件事：
    1. 正在跑的优先 —— 它的 messages 在开跑那一刻就定型了，不合并进去就看不到；
    2. 结果不依赖入参顺序 —— §3.2 的 `list_active_tasks` 只承诺 status 过滤，
       没承诺返回顺序，原来的 `active[-1]` 吃的是 SqliteSessionStore 的 `ORDER BY`；
    3. 不在 `_owned` 里的（孤儿）一个都不选。
    """
    old = _task("t-old", "s1", minutes=0)
    running = _task("t-running", "s1", minutes=1)
    newest = _task("t-newest", "s1", minutes=2)
    plane._owned |= {old.id, running.id, newest.id}
    plane._running.add(running.id)

    for order in ([old, running, newest], [newest, running, old], [running, newest, old]):
        assert plane._steer_target(order) is running

    # 没有在跑的 → 取最后创建的那个，同样不看入参顺序
    plane._running.clear()
    assert plane._steer_target([old, newest, running]) is newest
    assert plane._steer_target([newest, old, running]) is newest

    # 全是孤儿 → None，调用方按「新建 task」走
    plane._owned.clear()
    assert plane._steer_target([old, running, newest]) is None
    assert plane._steer_target([]) is None


# ---- ③ 任务没被领走时，队列谁来清 -----------------------------------------


async def test_stop_on_a_still_queued_task_clears_the_steer_queue(make_plane, store):
    """`!stop` 掉一个还在队列里排着的任务：`_dispatch_task` 在「已取消」那条提前
    return，worker 那圈 finally 压根不会执行。清理不放在最外层的话，`self._steer`
    里就永远留着这一条 —— 内存泄漏，用户那句追问也石沉大海。
    """
    plane = make_plane(ScriptedModel([]))       # 脚本是空的：模型被调用就该炸
    await plane.handle_event(make_event(text="按月画个图"))
    task = (await store.list_active_tasks(CHAT))[0]
    await _steer_once(plane)
    assert plane.pending_steer(task.id) == ["顺便加上同比"]

    await plane.handle_event(
        make_event(event_id="ev3", text="!stop", mentioned=False, message_id="om_3", thread_id=ROOT)
    )
    await plane.run_pending()

    assert (await store.get_task(task.id)).status is TaskStatus.cancelled
    assert plane.pending_steer(task.id) == []
    assert plane._steer == {}                    # 连空条目都不许留
    assert plane._owned == set()
    # !stop 就是让它停，这句追问不再自动变成新任务；但它在 transcript 里，
    # 下一次话题内追问会把它带进新任务的上下文。
    assert [t.content for t in await store.list_turns(task.session_id)] == [
        "按月画个图",
        "顺便加上同比",
    ]


async def test_queue_is_cleared_even_when_no_worker_is_configured(plane, store):
    """另一条提前 return：没配 model / worker（路由测试的默认装配）也要清干净。"""
    await plane.handle_event(make_event(text="按月画个图"))
    task = (await store.list_active_tasks(CHAT))[0]
    await _steer_once(plane)
    assert plane.pending_steer(task.id) == ["顺便加上同比"]

    await plane.run_pending()

    assert plane._steer == {}
    assert plane._owned == set()


async def test_steer_arriving_after_delivery_becomes_a_new_task(plane, store):
    """R6 的「否则新建 task 继续」：任务落终态之后到达的追问不该排队，
    也不该给死掉的 task_id 在 `defaultdict` 里新建一个永远没人 drain 的条目。
    """
    await plane.handle_event(make_event(text="第一问"))
    first = (await store.list_active_tasks(CHAT))[0]
    first.status = TaskStatus.delivered
    await store.update_task(first)

    await _steer_once(plane, text="再按季度画一张")

    active = await store.list_active_tasks(CHAT)
    assert len(active) == 1 and active[0].id != first.id
    assert active[0].session_id == first.session_id
    assert plane.counters["events.steer"] == 0
    assert plane._steer == {}
    assert plane.pending_steer(first.id) == []
