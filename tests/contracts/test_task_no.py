"""encode_task_no（aite/contracts/session.py）的契约测试。

验收依据：dev-spec-2026-09-09.md §3.1 的 session.py 代码块（docstring 里写死的四个向量）
以及 §6 T0 行「encode_task_no 四个向量」。
本文件只读契约、不改契约：任何断言失败都意味着契约实现被动过，而不是测试该放宽。
"""

from datetime import UTC, datetime

import pytest

import aite.contracts as contracts_pkg
from aite.contracts import Task, encode_task_no
from aite.contracts.session import _B32

# spec docstring 逐字节抄下来的四个向量：1→'#A1'，17→'#AH'，32→'#A10'，1000→'#AZ8'
SPEC_VECTORS = [
    (1, "#A1"),
    (17, "#AH"),
    (32, "#A10"),
    (1000, "#AZ8"),
]


@pytest.mark.parametrize(("n", "expected"), SPEC_VECTORS)
def test_encode_task_no_spec_vectors(n: int, expected: str) -> None:
    """spec 写死的四个向量必须逐字节相等（含 '#A' 前缀，不做 strip/大小写归一）。"""
    got = encode_task_no(n)
    assert got == expected, f"encode_task_no({n}) = {got!r}，spec 要求 {expected!r}"
    assert isinstance(got, str)


@pytest.mark.parametrize("n", [0, -1])
def test_encode_task_no_rejects_below_one(n: int) -> None:
    """n < 1 必须抛 ValueError（0 是边界，-1 是负数侧）；不许返回空串或降级。"""
    with pytest.raises(ValueError):
        encode_task_no(n)


@pytest.mark.parametrize("n", [-2, -32, -1000])
def test_encode_task_no_rejects_more_negatives(n: int) -> None:
    """负数一律 ValueError，避免 while n 循环对负数静默死循环/产生垃圾任务号。"""
    with pytest.raises(ValueError):
        encode_task_no(n)


def test_b32_alphabet_is_crockford() -> None:
    """字母表自洽性：Crockford base32 长度 32、无重复、全大写数字，且不含 I/L/O/U。

    这四个字母被 Crockford 排除是为了防止和 1/1/0/V 肉眼混淆；
    有人手滑把字母表改回标准 base32 时，这条会先红。
    """
    assert len(_B32) == 32, f"字母表长度应为 32，实际 {len(_B32)}"
    assert len(set(_B32)) == 32, "字母表内不得有重复字符"
    assert _B32 == "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
    for forbidden in ("I", "L", "O", "U"):
        assert forbidden not in _B32, f"Crockford base32 不得包含 {forbidden!r}"
    assert _B32[:10] == "0123456789"
    assert _B32[10:].isalpha() and _B32[10:].isupper()  # 字母段必须全大写字母


def test_encode_task_no_1_to_200_is_injective_and_prefixed() -> None:
    """1..200 的输出互不相同（单射，任务号不能撞车），且都以 '#A' 开头。"""
    outputs = [encode_task_no(n) for n in range(1, 201)]
    assert len(set(outputs)) == 200, "1..200 出现了重复任务号"
    for n, s in zip(range(1, 201), outputs, strict=True):
        assert s.startswith("#A"), f"encode_task_no({n}) = {s!r} 未以 '#A' 开头"
        assert len(s) > 2, f"encode_task_no({n}) 前缀之后必须还有编码体，实际 {s!r}"
        assert set(s[2:]) <= set(_B32), f"encode_task_no({n}) = {s!r} 含字母表外字符"


def test_encode_task_no_carry_boundaries() -> None:
    """进位边界：31/32/33 与 1023/1024 —— 32 进制换位处最容易写错。"""
    assert encode_task_no(31) == "#AZ"
    assert encode_task_no(32) == "#A10"
    assert encode_task_no(33) == "#A11"
    assert encode_task_no(1023) == "#AZZ"
    assert encode_task_no(1024) == "#A100"


def test_encode_task_no_deep_carry_boundaries() -> None:
    """更高位的进位边界：3→4 位（32767/32768）与 4→5 位（1048575/1048576）。

    原有的边界测试只盖到 2→3 位（1023/1024）。这里的期望值不是拿被测函数算出来的，
    是按 Crockford 表 32^3-1 = 32767 → 'ZZZ'、32^3 = 32768 → '1000' 手推后写死的。
    """
    assert encode_task_no(32767) == "#AZZZ"
    assert encode_task_no(32768) == "#A1000"
    assert encode_task_no(1048575) == "#AZZZZ"
    assert encode_task_no(1048576) == "#A10000"


def test_encode_task_no_body_has_no_leading_zero() -> None:
    """编码体首位不得是 '0'：n>=1 时最高位必然非零，出现前导零说明有人加了定长补齐。

    补齐会让 '#A01' 和 '#A1' 同时存在，任务号的字符串相等判定就废了。
    """
    for n in (1, 31, 32, 33, 1023, 1024, 32768, 1048576):
        body = encode_task_no(n)[2:]
        assert not body.startswith("0"), f"encode_task_no({n}) 的编码体 {body!r} 有前导零"


def test_encode_task_no_length_grows_only_at_powers_of_32() -> None:
    """编码体长度随 n 单调不减，且只在 32 的整数次幂处 +1（写死的位数表，不依赖被测实现）。"""
    for n, width in ((1, 1), (31, 1), (32, 2), (1023, 2), (1024, 3), (32767, 3), (32768, 4)):
        assert len(encode_task_no(n)) - 2 == width, f"encode_task_no({n}) 的编码体位数应为 {width}"
    widths = [len(encode_task_no(n)) for n in range(1, 2001)]
    assert widths == sorted(widths), "编码体长度出现了回退，说明不是标准 32 进制"


def test_task_no_survives_task_model_json_round_trip() -> None:
    """§3.1 的 Task.task_no 必须原样承载 encode_task_no 的输出（含 '#' 前缀）。

    这条把 encode_task_no 和它唯一的消费字段接起来：如果哪天 Task.task_no 被加上
    pattern/strip 之类的校验，'#A1' 会在这里先炸，而不是等到线上发任务号才发现。
    """
    now = datetime(2026, 9, 9, 12, 0, 0, tzinfo=UTC)
    task = Task(
        id="11111111-1111-4111-8111-111111111111",
        session_id="22222222-2222-4222-8222-222222222222",
        task_no=encode_task_no(1000),
        session_token="0" * 32,
        created_by="u1",
        created_at=now,
        updated_at=now,
    )
    assert task.task_no == "#AZ8"
    restored = Task.model_validate_json(task.model_dump_json())
    assert restored.task_no == "#AZ8"


def test_encode_task_no_is_reexported_from_package_root() -> None:
    """§3.1 要求 aite/contracts/__init__.py re-export 全部公开类型并写全 __all__。"""
    assert "encode_task_no" in contracts_pkg.__all__
    assert contracts_pkg.encode_task_no is encode_task_no
