"""能力副本（§3.7）、配置装配、令牌桶。

§3.7 的两点权限核实还没有结论，所以 `supports_passive_listen` 保持契约默认值 False，
但必须**运行时可改**，且改的是实例上的副本 —— `FEISHU_P0` 是全局共享的可变单例，
就地改它会污染所有引用方（包括别的 track 和 tests/contracts 的冻结值断言）。
"""
from __future__ import annotations

import pytest

from aite.adapters.feishu import FeishuPlatform, TokenBucket
from aite.contracts import FEISHU_P0, FeishuConfig, PlatformCapabilities

# ---------------------------------------------------------------------------
# §3.7：能力副本
# ---------------------------------------------------------------------------

def test_capabilities_default_to_the_frozen_feishu_p0() -> None:
    platform = FeishuPlatform(bot_open_id="ou_bot")
    assert platform.capabilities.model_dump() == FEISHU_P0.model_dump()
    # §3.7 结论未出：保持契约默认值。
    assert platform.capabilities.supports_passive_listen is False


def test_capabilities_are_a_per_instance_copy() -> None:
    """两个实例互不干扰，谁也不是 FEISHU_P0 本身。"""
    a = FeishuPlatform(bot_open_id="ou_bot")
    b = FeishuPlatform(bot_open_id="ou_bot")

    assert a.capabilities is not FEISHU_P0
    assert b.capabilities is not FEISHU_P0
    assert a.capabilities is not b.capabilities


def test_set_passive_listen_does_not_pollute_the_contract_singleton() -> None:
    """§3.7 核实后要改的是 adapter 实例，不是契约常量。"""
    before = FEISHU_P0.model_dump()

    platform = FeishuPlatform(bot_open_id="ou_bot")
    platform.set_passive_listen(True)

    assert platform.capabilities.supports_passive_listen is True
    assert FEISHU_P0.supports_passive_listen is False
    assert FEISHU_P0.model_dump() == before, "契约常量被就地改了"

    # 新建的实例仍然拿到干净的默认值。
    assert FeishuPlatform(bot_open_id="ou_bot").capabilities.supports_passive_listen is False


def test_capabilities_can_be_injected() -> None:
    custom = FEISHU_P0.model_copy(update={"outbound_rate_per_min": 10})
    platform = FeishuPlatform(bot_open_id="ou_bot", capabilities=custom)

    assert isinstance(platform.capabilities, PlatformCapabilities)
    assert platform.capabilities.outbound_rate_per_min == 10
    assert platform.capabilities is not custom, "注入的也要拷一份，别把调用方的对象攥在手里"


def test_outbound_rate_comes_from_the_capabilities() -> None:
    """§3.1：出站限速取 `outbound_rate_per_min`，不是另写一个魔数。"""
    platform = FeishuPlatform(bot_open_id="ou_bot")
    assert platform.api._bucket.rate_per_min == FEISHU_P0.outbound_rate_per_min == 60


# ---------------------------------------------------------------------------
# 从 FeishuConfig 装配
# ---------------------------------------------------------------------------

def test_from_config_reads_credentials_by_env_var_name() -> None:
    """契约里配置只存**环境变量名**不存值（§3.1 config.py 的注释）。"""
    cfg = FeishuConfig()
    env = {
        "FEISHU_APP_ID": "cli_from_env",
        "FEISHU_APP_SECRET": "secret_from_env",
        "FEISHU_BOT_OPEN_ID": "ou_bot_from_env",
    }

    platform = FeishuPlatform.from_config(cfg, env=env)

    assert platform.app_id == "cli_from_env"
    assert platform.bot_open_id == "ou_bot_from_env"
    assert platform.api.app_secret == "secret_from_env"
    assert platform.history_window == cfg.history_window


def test_from_config_honours_renamed_env_vars() -> None:
    cfg = FeishuConfig(app_id_env="MY_APP_ID", bot_open_id_env="MY_BOT")
    platform = FeishuPlatform.from_config(cfg, env={"MY_APP_ID": "cli_x", "MY_BOT": "ou_x"})

    assert platform.app_id == "cli_x"
    assert platform.bot_open_id == "ou_x"


def test_from_config_with_missing_env_does_not_explode() -> None:
    """启动期缺变量不该在装配时炸；真正打不通是调 API 时的事。"""
    platform = FeishuPlatform.from_config(FeishuConfig(), env={})
    assert platform.app_id == ""
    assert platform.bot_open_id == ""


# ---------------------------------------------------------------------------
# 令牌桶
# ---------------------------------------------------------------------------

async def test_token_bucket_allows_a_full_burst_then_paces(fake_clock) -> None:
    bucket = TokenBucket(60, clock=fake_clock, sleep=fake_clock.sleep)

    for _ in range(60):
        assert await bucket.acquire() == 0.0
    assert fake_clock.slept == []

    # 第 61 次要等 1 秒（60/分钟 = 每秒 1 个）。
    assert await bucket.acquire() == pytest.approx(1.0)
    assert fake_clock.slept == pytest.approx([1.0])


async def test_token_bucket_refills_over_time(fake_clock) -> None:
    bucket = TokenBucket(60, clock=fake_clock, sleep=fake_clock.sleep)
    for _ in range(60):
        await bucket.acquire()

    fake_clock.advance(10)
    assert bucket.tokens == pytest.approx(10.0)

    for _ in range(10):
        assert await bucket.acquire() == 0.0
    assert fake_clock.slept == []


async def test_token_bucket_never_exceeds_capacity(fake_clock) -> None:
    bucket = TokenBucket(60, clock=fake_clock, sleep=fake_clock.sleep)
    fake_clock.advance(3600)
    assert bucket.tokens == 60.0


def test_token_bucket_rejects_a_non_positive_rate() -> None:
    with pytest.raises(ValueError):
        TokenBucket(0)
