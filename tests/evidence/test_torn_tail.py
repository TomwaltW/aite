"""T21：崩溃残行 —— 写入侧能面对「上一条命写到一半」的 events.jsonl。

T18 在 `test_t18_crash_recovery.py` 上撞出两个真 bug 并钉住了当时的行为：冷 writer
一碰残行就抛 `ValidationError`（§3.3 的收尾链自锁 —— 写那条 failed 证据本身就是
炸的那个操作），热 writer 则把新证据直接接在残行后面、静默坏链。修法落在
`aite/evidence/writer.py`，那两条断言已经反过来了；这个文件钉的是修法的**边界**。

口径只有一条，**换行符是记录终止符**：

* 末尾一段没有换行符收尾 = 上一条命写到一半，那条记录从来没写完、也就从来没有效过；
* 中间某一行不合法 / hash 对不上 / seq 不连续 = 篡改或真损坏，**必须继续报 False**。

由此推出这个文件里最要紧的两组用例：`_tail_is_a_whole_record` 那一组（末段其实
写完了、只差换行符 → 一个字节都不许丢），和「篡改仍然 False」那一组（容错不许
扩大成什么都能吞）。
"""
import json
import logging
from pathlib import Path

import pytest

from aite.contracts import GENESIS, EvidenceEvent, EvidenceKind
from aite.evidence import FileEvidenceWriter

TASK = "t-torn"

#: 一段没写完的 JSON —— 进程在 `fh.write(...)` 中途没了的那一刻，文件末尾就长这样
HALF_LINE = '{"task_id": "' + TASK + '", "seq": 9, "kind": "model_ca'


@pytest.fixture
def writer(tmp_path) -> FileEvidenceWriter:
    return FileEvidenceWriter(tmp_path / "evidence")


def tear(writer: FileEvidenceWriter, task_id: str = TASK, text: str = HALF_LINE) -> None:
    """在 events.jsonl 末尾留下一段没有换行符收尾的字节。"""
    path = writer.events_path(task_id)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as fh:
        fh.write(text)


def lines_of(writer: FileEvidenceWriter, task_id: str = TASK) -> list[str]:
    return writer.events_path(task_id).read_text(encoding="utf-8").splitlines()


# ---- 残行的三种形状 --------------------------------------------------------


async def test_torn_tail_after_several_whole_records(writer):
    """最常见的形状：几条完整记录之后跟着半行。收拾掉，链从它之前接着长。"""
    for i in range(3):
        await writer.append(TASK, EvidenceKind.model_call, {"step": i})
    tear(writer)

    fresh = FileEvidenceWriter(writer.task_dir(TASK).parent)   # 重启后的新进程
    ev = await fresh.append(TASK, EvidenceKind.failed, {"why": "crash"})

    assert ev.seq == 3                                          # 半行不占号
    assert fresh.verify(TASK) is True
    assert [EvidenceEvent.model_validate_json(x).seq for x in lines_of(writer)] == [0, 1, 2, 3]


async def test_events_file_that_is_only_a_torn_line(writer):
    """整个文件就是半行 —— 崩在第一条证据上。截完是空文件，新链从 seq 0 起。"""
    tear(writer)
    assert writer.events_path(TASK).read_bytes() == HALF_LINE.encode("utf-8")

    ev = await writer.append(TASK, EvidenceKind.task_created, {"a": 1})

    assert (ev.seq, ev.prev_hash) == (0, GENESIS)
    assert writer.counters["evidence.torn_tail_dropped"] == 1
    assert writer.verify(TASK) is True
    assert len(lines_of(writer)) == 1


async def test_empty_events_file_is_not_a_torn_tail(writer):
    """0 字节的文件没有残行 —— 别把「还没写过」当成崩溃现场。"""
    path = writer.events_path(TASK)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.touch()

    ev = await writer.append(TASK, EvidenceKind.task_created, {"a": 1})

    assert ev.seq == 0
    assert dict(writer.counters) == {}                          # 什么都没收拾
    assert writer.verify(TASK) is True


async def test_torn_tail_cut_inside_a_multibyte_character(writer):
    """残行截在一个 UTF-8 中文字符中间。

    这一条不是凑数：按字符读全文的话，`read_text` 会先炸在 `UnicodeDecodeError`
    上 —— 那是 `ValueError` 的子类，`append` 照样抛，等于没修。
    """
    await writer.append(TASK, EvidenceKind.task_created, {"a": 1})
    path = writer.events_path(TASK)
    with path.open("ab") as fh:
        fh.write('{"task_id": "x", "payload": {"b": "文'.encode()[:-1])   # 「文」被砍掉一截

    ev = await writer.append(TASK, EvidenceKind.failed, {"why": "crash"})

    assert ev.seq == 1
    assert writer.counters["evidence.torn_tail_dropped"] == 1
    assert writer.verify(TASK) is True


# ---- 只差一个换行符：一个字节都不许丢 --------------------------------------


async def test_a_record_missing_only_its_newline_is_kept(writer):
    """末段其实整条写完了，只差那个换行符 —— 补上，不许当残行截掉。

    收拾的原则是「只丢弃 `verify()` 本来就会拒绝的东西」。这个形状 `verify()`
    本来就认（`splitlines()` 照样切得出来、链也闭合），截掉它就是真丢了一条证据。
    `write` 断在最后一个字节前的概率不高，但磁盘满那一路想断哪就断哪。
    """
    await writer.append(TASK, EvidenceKind.task_created, {"a": 1})
    second = await writer.append(TASK, EvidenceKind.model_call, {"b": "文"})

    path = writer.events_path(TASK)
    raw = path.read_bytes()
    with path.open("r+b") as fh:
        fh.truncate(len(raw) - 1)                               # 只砍掉末尾的换行符
    assert writer.verify(TASK) is True, "前提：这个形状 verify 本来就是 True"

    third = await FileEvidenceWriter(writer.task_dir(TASK).parent).append(
        TASK, EvidenceKind.delivered, {"c": 3}
    )

    assert third.seq == 2 and third.prev_hash == second.hash    # 第 2 条还在链上
    assert writer.verify(TASK) is True                          # 收拾前后答案不变
    assert len(lines_of(writer)) == 3


async def test_a_whole_record_missing_its_newline_is_counted_separately(writer):
    """补换行符和截残行是两件事，留痕也分开计 —— 排障时得分得清丢没丢东西。"""
    await writer.append(TASK, EvidenceKind.task_created, {"a": 1})
    path = writer.events_path(TASK)
    with path.open("r+b") as fh:
        fh.truncate(path.stat().st_size - 1)

    await writer.append(TASK, EvidenceKind.delivered, {"c": 3})

    assert writer.counters["evidence.torn_tail_kept"] == 1
    assert writer.counters["evidence.torn_tail_dropped"] == 0


async def test_a_tail_that_parses_but_does_not_fit_the_chain_is_dropped(writer):
    """末段是合法的 EvidenceEvent，但 hash 接不住前面 —— 那不是「写完了」，是垃圾。

    只看「解析得了吗」不够：判据必须是 `verify()` 那把尺子，否则任何一段合法 JSON
    追加到末尾都会被当成有效证据留下来。
    """
    first = await writer.append(TASK, EvidenceKind.task_created, {"a": 1})
    forged = first.model_copy(update={"seq": 1, "prev_hash": first.hash, "hash": "0" * 64})
    tear(writer, text=forged.model_dump_json())                 # 合法 JSON，hash 是编的

    ev = await writer.append(TASK, EvidenceKind.failed, {"why": "crash"})

    assert ev.seq == 1                                          # 编的那条被截掉了
    assert writer.counters["evidence.torn_tail_dropped"] == 1
    assert writer.verify(TASK) is True


# ---- 篡改检测：容错不许扩大成什么都能吞 ------------------------------------


def tamper_middle(writer: FileEvidenceWriter, task_id: str = TASK) -> None:
    """把中间那一行的 payload 改掉（行还是合法 JSON，只是 hash 对不上了）。"""
    path = writer.events_path(task_id)
    lines = path.read_text(encoding="utf-8").splitlines()
    row = json.loads(lines[1])
    row["payload"] = {"有人": "动过这一行"}
    lines[1] = json.dumps(row, ensure_ascii=False)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


async def _three_events(writer) -> None:
    for i in range(3):
        await writer.append(TASK, EvidenceKind.model_call, {"step": i})


async def test_a_tampered_middle_line_still_fails_verify(writer):
    """中间行被改坏 → `verify()` 永远 False。这是这套证据链存在的理由。"""
    await _three_events(writer)
    tamper_middle(writer)

    assert writer.verify(TASK) is False
    assert FileEvidenceWriter(writer.task_dir(TASK).parent).verify(TASK) is False


async def test_a_torn_tail_does_not_launder_a_tampered_middle_line(writer):
    """残行 + 中间被改坏：收拾掉残行、接着往下写，篡改仍然报 False。

    最该防的一条 —— 「崩溃恢复」不许变成把中间的篡改一起洗白。改坏的那一行本身
    还是合法 JSON（只有 payload 与 hash 对不上），所以 `append` 这一路不抛，
    正好能验到「写得下去，但链依然是坏的」。
    """
    await _three_events(writer)
    tamper_middle(writer)
    tear(writer)

    fresh = FileEvidenceWriter(writer.task_dir(TASK).parent)
    ev = await fresh.append(TASK, EvidenceKind.failed, {"why": "crash"})

    assert ev.seq == 3                                          # 收拾只碰末尾那半行
    assert fresh.counters["evidence.torn_tail_dropped"] == 1
    assert fresh.verify(TASK) is False                          # 中间那一行还是坏的


async def test_append_refuses_to_extend_a_tampered_chain(writer):
    """中间行不合法时 `append` / `finalize` 仍然抛。

    不许「无脑容错」：一条已经不可信的链上继续追加，等于给篡改盖章。
    这里的文件是以换行符正常收尾的 —— 没有残行，坏的就是中间那一行。
    """
    await _three_events(writer)
    path = writer.events_path(TASK)
    lines = path.read_text(encoding="utf-8").splitlines()
    lines[1] = '{"task_id": "' + TASK + '", "kind": 坏掉的'      # 中间行连 JSON 都不是
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")

    fresh = FileEvidenceWriter(writer.task_dir(TASK).parent)
    with pytest.raises(ValueError):                             # pydantic ValidationError
        await fresh.append(TASK, EvidenceKind.failed, {"why": "crash"})
    with pytest.raises(ValueError):
        await FileEvidenceWriter(writer.task_dir(TASK).parent).finalize(TASK, {})


async def test_dropping_a_middle_line_still_fails_verify(writer):
    """整行被删（seq 断了）也不许被当成「残行」收拾掉。"""
    await _three_events(writer)
    path = writer.events_path(TASK)
    lines = path.read_text(encoding="utf-8").splitlines()
    path.write_text(lines[0] + "\n" + lines[2] + "\n", encoding="utf-8")

    fresh = FileEvidenceWriter(writer.task_dir(TASK).parent)
    await fresh.append(TASK, EvidenceKind.failed, {"why": "crash"})

    assert dict(fresh.counters) == {}                           # 文件以换行符收尾，没残行
    assert fresh.verify(TASK) is False


# ---- finalize -------------------------------------------------------------


async def test_finalize_after_a_torn_tail_writes_a_manifest(writer):
    """崩溃后第一个被调到的常常是 `finalize` —— 它也要走收拾，而不是抛。"""
    last = None
    for i in range(3):
        last = await writer.append(TASK, EvidenceKind.model_call, {"step": i})
    tear(writer)

    fresh = FileEvidenceWriter(writer.task_dir(TASK).parent)
    root = await fresh.finalize(TASK, {"session_id": "s-1", "task_no": "#A9"})

    assert root == last.hash                                    # 半行不改 root_hash
    manifest = json.loads(fresh.manifest_path(TASK).read_text(encoding="utf-8"))
    assert manifest["event_count"] == 3                         # 也不算进 event_count
    assert manifest["root_hash"] == root
    assert fresh.verify(TASK) is True


async def test_finalize_on_a_file_that_is_only_a_torn_line(writer):
    """一条完整证据都没有、只有半行：root_hash 是 GENESIS，不是异常。"""
    tear(writer)

    assert await writer.finalize(TASK, {}) == GENESIS
    manifest = json.loads(writer.manifest_path(TASK).read_text(encoding="utf-8"))
    assert manifest["event_count"] == 0
    assert writer.events_path(TASK).read_bytes() == b""


# ---- 留痕与成本 -----------------------------------------------------------


async def test_healing_logs_a_warning_with_the_torn_bytes(writer, caplog):
    """静默截掉一条记录跟静默坏链一样难查：字节数和残行原文都要进日志。"""
    await writer.append(TASK, EvidenceKind.task_created, {"a": 1})
    tear(writer)

    with caplog.at_level(logging.WARNING, logger="aite.evidence"):
        await writer.append(TASK, EvidenceKind.failed, {"why": "crash"})

    warnings = [r for r in caplog.records if r.levelno == logging.WARNING]
    assert len(warnings) == 1
    message = warnings[0].getMessage()
    assert str(len(HALF_LINE)) in message                        # 截掉了多少字节
    assert HALF_LINE[:30] in message                             # 残行原文（截断后的前缀）
    assert str(writer.events_path(TASK)) in message


async def test_a_healthy_file_is_never_read_whole_on_append(writer, monkeypatch):
    """健康文件上 `append` 只探一个字节，不读全文 —— 这条路每条证据都会走。

    钉住成本：哪天有人把 O(1) 的探测换成「每次读一遍再判断」，这里会红。
    `_heal_torn_tail` 是本模块唯一用 `read_bytes` 的地方。
    """
    calls = 0
    original = Path.read_bytes

    def counting(self):
        nonlocal calls
        calls += 1
        return original(self)

    monkeypatch.setattr(Path, "read_bytes", counting)
    for i in range(5):
        await writer.append(TASK, EvidenceKind.model_call, {"step": i})

    assert calls == 0
    assert dict(writer.counters) == {}
