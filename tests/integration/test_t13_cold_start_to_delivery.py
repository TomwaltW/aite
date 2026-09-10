"""第 6 组：冷启动 → 交付的进程级贯通（owner: T13）。

前五组把 `run_app` 的**停机**面钉死了：退出顺序、宽限期、证据落盘、跨进程续接。
「起飞 → 干活 → 交付」这条主干在这一层一条都没有 —— 它一直被测在**别的层**：
`aite/evals/` 那 10 个场景自己拼 plane + worker（不走 `run_app`），
`tests/worker/` 只看 worker 内部。于是 `run_app` 今天只被「怎么停」验过，
没被「怎么跑起来并交付」验过。

而 §2.4 的 M1（@ 一下有反应）和 M3（CSV 画图、卡片更新、产物回线程）
在真机上走的正是这条路。真机上出问题没有断言告诉你哪一行红了，
只有群里一条没回的消息 —— 这一组就是把这条路钉住。

**判据不是评测那 10 个场景的判据的副本。** 同一件事在两层都红时，两层要能把
责任分开：评测红 = 行为本身错了；这里红 = 行为对，但 `run_app` 的装配没把它接上
（落盘目录少 mkdir、`session_token` 没登记给 Gateway、沙箱没一路传下去、
收尾把还没落盘的东西吃掉了）。所以这里的断言尽量挑**只有装配才决定得了**的那些：
沙箱同一只、产物从 Gateway 那只沙箱里取、目录真的被建出来、收尾之后盘上还在。

一趟飞行覆盖八个节点，一个节点一条用例，每条都从 `build_app` + `run_app` 走一遍：

    1 冷启动建目录  2 R7 ack + #A1   3 W3 只有一张卡   4 W4 原地更新
    5 工具链落到沙箱 6 W5 产物与交付  7 evidence 收口   8 停机还沙箱

落盘一律走 `tmp_path`（口径见 `test_evidence_stays_inside_tmp_path`），
仓库的 data/ 一个字节都不许写。
"""
import hashlib
from contextlib import asynccontextmanager
from dataclasses import dataclass
from pathlib import Path

import pytest
from app_under_test import AiteApp, build_app
from integration_fakes import (
    CHAT,
    REPO_ROOT,
    ClosableFakeSandbox,
    GatedPlatform,
    RecordingModel,
    evidence_task_dirs,
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
    AiteConfig,
    Attachment,
    EvidenceKind,
    Task,
    TaskStatus,
    chain_hash,
    payload_hash_of,
)
from aite.evidence import FileEvidenceWriter
from aite.testing import CSV_SAMPLE, PNG_MAGIC

#: R7 把这条消息定成话题 root（`plane._new_session(thread_id=ev.anchor.message_id)`），
#: 所以它同时是「附件挂在哪条消息上」和「产物该回到哪里」的答案。
ROOT_MSG = "om_1"
FILE_KEY = "file_sales_csv"
INBOX_PATH = "/work/in/file_sales_csv"
ARTIFACT_PATH = "/work/out.png"

CSV_BYTES = CSV_SAMPLE.encode("utf-8")

#: 一整条真机出牌：3 项 checklist → 下附件 → 画图 → 列文件 → 逐项 check → 带产物 final。
#: 中间穿插 checklist_check 是为了让 W4 的「原地更新」有真东西可更新 ——
#: 卡片内容每一步都在变，不是同一份快照被重推三次。
SCRIPT = [
    tool_step("checklist_add", {"items": ["下载数据", "画图", "交付"]}),
    tool_step("download_attachment", {"file_key": FILE_KEY}),
    tool_step("checklist_check", {"id": "c1"}),
    tool_step(
        "run_python",
        {
            "code": (
                "import pandas as pd, matplotlib\n"
                'matplotlib.use("Agg")\n'
                "import matplotlib.pyplot as plt\n"
                f'df = pd.read_csv("{INBOX_PATH}")\n'
                'df.plot(x="month", y="amount")\n'
                f'plt.savefig("{ARTIFACT_PATH}")\n'
            ),
            "timeout_sec": 120,
        },
    ),
    tool_step("checklist_check", {"id": "c2"}),
    tool_step("list_files", {}),
    tool_step("checklist_check", {"id": "c3"}),
    final_step("月度趋势图见附件。", artifacts=[{"path": ARTIFACT_PATH, "title": "月度趋势"}]),
]

#: 代码里出现 savefig 就当画好了图，往 /work/out.png 写一张真 PNG（前 8 字节是魔数）。
EXEC_SCRIPT = [
    {
        "match": "savefig",
        "exit_code": 0,
        "stdout": "saved /work/out.png",
        "writes": {ARTIFACT_PATH: "builtin:png"},
    }
]


def _mention_with_csv():
    """M1 + M3 的入口事件：@ 了 Aite，并且带一个 CSV 附件。"""
    return make_event(
        text="把这个 CSV 画成月度趋势图",
        message_id=ROOT_MSG,
        attachments=[
            Attachment(
                kind="file",
                file_key=FILE_KEY,
                message_id=ROOT_MSG,
                name="sales.csv",
                mime="text/csv",
                size=len(CSV_BYTES),
            )
        ],
    )


@dataclass
class Flight:
    """跑完一趟之后的现场。`task` 是刚建出来那一刻的快照，终态一律从盘上读。"""

    config: AiteConfig
    app: AiteApp
    platform: GatedPlatform
    model: RecordingModel
    sandbox: ClosableFakeSandbox
    task: Task

    @property
    def sandbox_id(self) -> str:
        """这趟飞行一共开过几只沙箱 —— 答案必须是一只，不然产物就取错了地方。"""
        assert len(self.sandbox.boxes) == 1, (
            f"一个任务只该有一只沙箱，实际开了 {sorted(self.sandbox.boxes)}。"
            "多开一只通常意味着 `_fetch_artifact` 没从 Gateway 手里要 sandbox_id，"
            "而是自己 acquire 了一个全新的空容器 —— 那里面永远没有产物。"
        )
        return next(iter(self.sandbox.boxes))

    @property
    def card_id(self) -> str:
        assert len(self.platform.cards) == 1, f"应当只有一张卡片，实际 {sorted(self.platform.cards)}"
        return next(iter(self.platform.cards))


@asynccontextmanager
async def fly(config: AiteConfig):
    """从空的 data/ 起飞，跑完一整个任务，在**还没停机**的时候把现场交出来。

    `yield` 的那一刻任务已经彻底收完：`AppWorker.in_flight` 空了 = `worker.run()`
    真的返回了，于是 evidence 已 finalize、沙箱已还、任务终态已进库。只等
    `platform.count("send_text")` 是不够的 —— 那时 `_deliver` 还没走到 `_finish`。
    """
    platform = GatedPlatform(files={(ROOT_MSG, FILE_KEY): CSV_BYTES})
    model = RecordingModel(SCRIPT)
    sandbox = ClosableFakeSandbox(exec_script=EXEC_SCRIPT)
    app = build_app(config, platform=platform, model=model, sandbox=sandbox)

    async with running_app(app):
        await platform.emit(_mention_with_csv())

        active = await app.store.list_active_tasks(CHAT)
        assert len(active) == 1, f"@ 一次只该建一个任务，实际 {len(active)} 个"
        task = active[0]

        await wait_until(lambda: platform.count("send_text") == 1, what="W5 的 send_text（交付回帖）")
        await wait_until(lambda: not app.worker.in_flight, what="worker.run() 返回（收尾做完）")

        yield Flight(
            config=config, app=app, platform=platform, model=model, sandbox=sandbox, task=task
        )


# --------------------------------------------------------------------------
# 1 冷启动
# --------------------------------------------------------------------------

async def test_cold_start_creates_every_storage_dir(cold_config, tmp_path):
    """`storage.*` 的父目录一个都不存在时，起飞要把它们建出来。

    判据取自 `scripts/preflight.py` 第 7 项对人说的那句话：目录不在不算 FAIL，
    因为「起飞时自动 mkdir」。真机第一次跑就是这个状态 —— 仓库里没有 data/。
    这条一红就说明那句话是空头支票，用户会在起飞的第一秒撞上。
    """
    storage = cold_config.storage
    data_root = Path(storage.sqlite_path).parent
    assert not data_root.exists(), "这条用例的前提是 data/ 还没建，fixture 给错了"

    async with fly(cold_config) as flight:
        assert data_root.is_dir(), f"{storage.sqlite_path} 的父目录没被建出来"
        assert Path(storage.evidence_dir).is_dir(), f"{storage.evidence_dir} 没被建出来"
        assert Path(storage.artifacts_dir).is_dir(), (
            f"{storage.artifacts_dir} 没被建出来。三个落盘目录里只有它落了空 —— "
            "preflight 第 7 项对人承诺的是「起飞时自动 mkdir」，起飞这一层就得真的建。"
        )

        # 目录建了还不够，东西得真落进去
        assert Path(storage.sqlite_path).is_file(), "SQLite 文件没落盘"
        assert evidence_task_dirs(cold_config) == [flight.task.id]

    # 整轨硬约束：一个字节都不许写进仓库的 data/。
    # 这条用例偏偏自己造了一层 data/，所以不能照搬「路径里不含 /data/」那种字面判断 ——
    # 判据是**落点在哪**：全都在 tmp_path 下、一个都不在仓库里。
    for path in (storage.sqlite_path, storage.evidence_dir, storage.artifacts_dir):
        assert Path(path).is_relative_to(tmp_path), f"{path} 落到了 tmp_path 外面"
        assert not Path(path).is_relative_to(REPO_ROOT), f"{path} 落进了仓库"


# --------------------------------------------------------------------------
# 2 R7：@ 一下有反应
# --------------------------------------------------------------------------

async def test_mention_gets_an_ack_then_a_session_and_task(cold_config):
    """M1 的全部内容：@ 了就要有 ack 表情，并且建出会话 + `#A1` 任务。

    ack 必须**早于**卡片：R7 的第一反应是让人知道「收到了」，卡片是第一个工具调用
    之前才发的（W3）。这两件事在真机上隔着一次模型往返，顺序反了人就要干等。
    """
    async with fly(cold_config) as flight:
        assert flight.platform.reactions == [(ROOT_MSG, "ack")], (
            f"R7 应当给 @ 的那条消息加一个 ack，实际 {flight.platform.reactions}"
        )

        methods = flight.platform.calls.methods()
        assert methods.index("add_reaction") < methods.index("send_card"), (
            f"ack 要发生在卡片之前，实际顺序：{methods}"
        )

        assert flight.task.task_no == "#A1", (
            f"租户里的第一个任务应当是 #A1，实际 {flight.task.task_no}"
        )
        assert flight.task.created_by == "ou_user"

        session = await flight.app.store.get_session(flight.task.session_id)
        assert session is not None, "R7 建了任务却没有会话"
        assert session.chat_id == CHAT
        assert session.anchor.thread_id == ROOT_MSG, (
            "@ 的那条消息就是话题 root（R7），产物和回帖都要回到这里"
        )


# --------------------------------------------------------------------------
# 3 W3：只有一张卡
# --------------------------------------------------------------------------

async def test_one_card_is_sent_before_the_first_tool_call(cold_config):
    """W3：第一个非 `final` 的 tool_call 之前先 `send_card`，而且**只有一张**。

    「在第一个工具之前」这件事在平台这一层是看得见的：`download_attachment` 会去调
    `platform.download_file`，所以 `send_card` 必须排在它前面。
    """
    async with fly(cold_config) as flight:
        assert flight.platform.count("send_card") == 1, (
            f"整趟只该发一张卡片，实际 {flight.platform.count('send_card')} 张"
        )
        assert flight.platform.card_count == 1

        methods = flight.platform.calls.methods()
        assert methods.index("send_card") < methods.index("download_file"), (
            f"W3：卡片要发在第一个工具调用之前，实际顺序：{methods}"
        )

        first = flight.platform.card_snapshots(flight.card_id)[0]
        assert first.status == "working"
        assert first.task_no == "#A1"


# --------------------------------------------------------------------------
# 4 W4：原地更新，不新增消息
# --------------------------------------------------------------------------

async def test_card_is_updated_in_place_and_nothing_new_is_posted(cold_config):
    """W4：过程中 `update_card` ≥3 次，全落在同一张卡上，群里不多出任何一条消息。

    `FakePlatform.update_card` 只认已经存在的 card_id，所以「更新的是同一张卡」
    这件事在替身那一层就被钉住了；这里补的是数量与内容：清单真的从 todo 走到 done。
    """
    async with fly(cold_config) as flight:
        assert flight.platform.update_count >= 3, (
            f"清单一路在变，update_card 至少该有 3 次，实际 {flight.platform.update_count} 次"
        )
        assert {c.kwargs["card_id"] for c in flight.platform.calls.of("update_card")} == {
            flight.card_id
        }, "所有更新必须落在同一张卡片上"

        # 群里的出站消息只有：1 张卡 + 1 个产物 + 1 条回帖。没有第二条卡片、没有中途播报。
        assert flight.platform.count("send_card") == 1
        assert flight.platform.count("send_file") == 1
        assert flight.platform.count("send_text") == 1

        snapshots = flight.platform.card_snapshots(flight.card_id)
        assert len(snapshots) >= 4, f"一次 send_card + ≥3 次 update，实际 {len(snapshots)} 份快照"
        assert [i.state for i in snapshots[0].items] == [], "第一张卡发出去时清单还是空的"

        last = snapshots[-1]
        assert last.status == "delivered", f"收尾那次更新要把卡片置成 delivered，实际 {last.status}"
        assert [i.text for i in last.items] == ["下载数据", "画图", "交付"]
        assert [i.state for i in last.items] == ["done", "done", "done"]


# --------------------------------------------------------------------------
# 5 工具链：一路落到同一只沙箱
# --------------------------------------------------------------------------

async def test_the_tool_chain_lands_in_one_sandbox(cold_config):
    """`download_attachment` → `run_python` → 产出文件 → `list_files` 看得见。

    这条最能区分「行为对」与「装配对」：`P0ToolGateway` 的 token 校验是失败关闭的，
    组装没把 `Task.session_token` 登记进去的话，每个工具调用都会是 `denied`，
    而任务照样会 delivered —— 群里只是收不到图。所以这里逐个查 tool_result 的 ok。
    """
    async with fly(cold_config) as flight:
        results = {
            e.payload["name"]: e.payload
            for e in read_events(cold_config, flight.task.id)
            if e.kind is EvidenceKind.tool_result
        }
        for name in ("download_attachment", "run_python", "list_files"):
            assert name in results, f"{name} 一次都没跑到，实际跑了：{sorted(results)}"
            assert results[name]["ok"] is True, (
                f"{name} 没跑成：{results[name]['error']}。"
                "denied 的话多半是组装没把 Task.session_token 登记给 Gateway"
            )

        # 三个工具用的是同一只沙箱，产物就在里面
        sid = flight.sandbox_id
        files = flight.sandbox.all_files()[sid]
        assert INBOX_PATH in files, f"附件没进沙箱，/work 下只有：{sorted(files)}"
        assert files[INBOX_PATH] == len(CSV_BYTES)
        assert ARTIFACT_PATH in files, f"run_python 没产出图，/work 下只有：{sorted(files)}"

        listed = flight.sandbox.calls.of("list_files")
        assert [c.kwargs["sandbox_id"] for c in listed] == [sid], (
            f"list_files 应当只列那一只沙箱，实际 {listed}"
        )

        # 产物真的进了模型的下一轮上下文 —— 模型是「看见」了它才敢 final 的
        assert any(ARTIFACT_PATH in text for text in flight.model.prompt_texts()), (
            "沙箱里产出的文件没有出现在任何一轮模型上下文里"
        )


# --------------------------------------------------------------------------
# 6 W5：产物与交付
# --------------------------------------------------------------------------

async def test_final_artifacts_go_back_to_the_thread_then_the_reply(cold_config):
    """W5：`final(artifacts)` → `get_file` → `send_file`（回话题 root）→ 再 `send_text`。

    M3 在真机上就卡在这一步：图发不回去的话，群里只会看到一句「图见附件」而没有附件。
    所以这里连字节都验：发出去的必须是沙箱里那张 PNG，不是别的什么东西。
    """
    async with fly(cold_config) as flight:
        methods = flight.platform.calls.methods()
        assert methods.index("send_file") < methods.index("send_text"), (
            f"产物要先于回帖发出去，实际顺序：{methods}"
        )

        sent = flight.platform.sent_files[0]
        assert sent.data[:8] == PNG_MAGIC, "发回去的不是沙箱里那张 PNG"
        assert sent.data == flight.sandbox.boxes[flight.sandbox_id].files[ARTIFACT_PATH]
        assert sent.name == "out.png"
        assert sent.mime == "image/png"
        assert sent.chat_id == CHAT
        assert sent.reply_to == ROOT_MSG, (
            f"产物要回到话题 root（{ROOT_MSG}），实际发到了 {sent.reply_to}"
        )

        # 产物是从 **Gateway 那只**沙箱里取的。自己新 acquire 一个的话拿到的是空容器，
        # `_fetch_artifact` 会静默跳过，回帖变成「产物 … 未找到」而任务照样 delivered。
        fetched = flight.sandbox.calls.of("get_file")
        assert [(c.kwargs["sandbox_id"], c.kwargs["path"]) for c in fetched] == [
            (flight.sandbox_id, ARTIFACT_PATH)
        ], f"产物应当从任务那只沙箱里取一次，实际 {fetched}"

        reply = flight.platform.sent_texts[0]
        assert reply.text == "月度趋势图见附件。", "回帖不该带「产物未找到」之类的尾巴"
        assert reply.reply_to == ROOT_MSG and reply.in_thread is True

        # evidence 里的 artifact 记的就是真发出去的那份字节，不是「打算发」的元数据
        artifacts = [
            e.payload
            for e in read_events(cold_config, flight.task.id)
            if e.kind is EvidenceKind.artifact
        ]
        assert len(artifacts) == 1, f"发了一个产物就该有一条 artifact 证据，实际 {len(artifacts)} 条"
        assert artifacts[0]["title"] == "月度趋势"
        assert artifacts[0]["mime"] == "image/png"
        assert artifacts[0]["size"] == len(sent.data)
        assert artifacts[0]["sha256"] == hashlib.sha256(sent.data).hexdigest()

        # 库里的终态：从**新连接**读，确认真落盘了而不是只在内存里
        delivered = await read_task_from_disk(cold_config, flight.task.id)
        assert delivered is not None
        assert delivered.status is TaskStatus.delivered
        assert delivered.result_summary == "月度趋势图见附件。"
        assert delivered.steps == len(SCRIPT)


# --------------------------------------------------------------------------
# 7 evidence：链条收口
# --------------------------------------------------------------------------

async def test_the_evidence_chain_closes_on_this_path(cold_config):
    """这条贯通路径跑完之后，证据链要能收口并自证。

    与第 2 组（`test_t8_evidence_on_disk.py`）的分工：那边验的是链条与篡改检测本身，
    这里验的是**这条路上该有的证据一条不少**，而且每个 tool_call 都配得上一个
    tool_result —— 工具在半路被吞掉的话，链条照样能通过 verify，但少一对。
    """
    async with fly(cold_config) as flight:
        task_id = flight.task.id
        events = read_events(cold_config, task_id)

        prev = GENESIS
        for e in events:
            assert e.payload is not None, f"seq={e.seq} 的 payload 没内联"
            assert e.payload_hash == payload_hash_of(e.payload)
            assert e.prev_hash == prev
            assert e.hash == chain_hash(prev, e.payload_hash)
            prev = e.hash

        kinds = [e.kind for e in events]
        assert kinds[0] is EvidenceKind.task_created
        assert kinds[1] is EvidenceKind.event_received
        assert kinds[-1] is EvidenceKind.delivered
        assert EvidenceKind.artifact in kinds, "产物发出去了却没写 artifact 证据"

        # 每个 tool_call 都要有配对的 tool_result（本地 checklist_* 与 Gateway 工具一视同仁）。
        # `final` 是唯一的例外：它不回结果给模型，它的「结果」就是最后那条 delivered。
        calls = [
            e.payload["call_id"]
            for e in events
            if e.kind is EvidenceKind.tool_call and e.payload["name"] != "final"
        ]
        done = [e.payload["call_id"] for e in events if e.kind is EvidenceKind.tool_result]
        assert calls, "一个 tool_call 证据都没有"
        assert calls == done, f"tool_call 与 tool_result 没配上：发起 {calls}，回收 {done}"

        finals = [
            e for e in events if e.kind is EvidenceKind.tool_call and e.payload["name"] == "final"
        ]
        assert len(finals) == 1, f"final 只该出一次，实际 {len(finals)} 次"

        manifest = read_manifest(cold_config, task_id)
        assert manifest["root_hash"] == events[-1].hash
        assert manifest["event_count"] == len(events)
        assert manifest["task_no"] == "#A1"
        assert manifest["contract_version"] == CONTRACT_VERSION
        assert FileEvidenceWriter(cold_config.storage.evidence_dir).verify(task_id) is True

        from_disk = await read_task_from_disk(cold_config, task_id)
        assert from_disk is not None
        assert from_disk.evidence_root_hash == manifest["root_hash"], "库里与盘上的 root_hash 漂了"


# --------------------------------------------------------------------------
# 8 停机
# --------------------------------------------------------------------------

async def test_shutdown_after_delivery_returns_the_sandbox(cold_config):
    """队列空了之后停机：沙箱还回去，库关掉，盘上的东西一样不少。

    与第 3 组的分工：那边停的是**在跑**的任务（走 `plane.join()` 的等待分支），
    这里停的是已经交付完的系统 —— 沙箱在任务收尾时就该还了，`aclose()` 只是兜底。
    真机上这条一红的表现是容器泄漏：任务早结束了，容器还占着，直到 reaper 空闲超时。
    """
    async with fly(cold_config) as flight:
        sid = flight.sandbox_id
        assert flight.sandbox.alive == [], (
            f"任务收尾（`_finish`）就该把沙箱还掉，实际还活着：{flight.sandbox.alive}"
        )
        assert flight.sandbox.released_ids == [sid]

    # ── 退出之后 ────────────────────────────────────────────────
    assert flight.platform.stopped is True
    assert flight.sandbox.aclose_calls == 1, "C-TΩ-1 的退出序列里有 sandbox.aclose()"
    assert flight.sandbox.released_ids == [sid], "已经还过的沙箱不该被重复 release"

    with pytest.raises(RuntimeError):        # store.close() 调过了
        await flight.app.store.get_task(flight.task.id)

    # 进程没了，盘上的交付物还在 —— 这才是「收干净」而不是「擦干净」
    assert manifest_path(cold_config, flight.task.id).is_file()
    assert read_manifest(cold_config, flight.task.id)["task_no"] == "#A1"
