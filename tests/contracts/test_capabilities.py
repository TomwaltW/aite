"""平台能力面 + 配置形状的冻结测试（dev-spec-2026-09-09 §3.1 capabilities.py / config.py）。

验的是「契约的默认取值不许漂」：谁改了 FEISHU_P0 的任一字段、改了 Platform 的取值集合、
或改了 AiteConfig 的关键默认值，这里立刻红。
"""

import re
from typing import get_args

import pytest
from pydantic import ValidationError

from aite.contracts import (
    FEISHU_P0,
    AiteConfig,
    FeishuConfig,
    ModelConfig,
    Platform,
    PlatformCapabilities,
    SandboxConfig,
    StorageConfig,
    WorkerConfig,
)

# §3.1 里 FEISHU_P0 的逐字段取值，写死在这里当冻结判据。
EXPECTED_FEISHU_P0: dict[str, object] = {
    "platform": "feishu",
    "supports_thread": True,
    "supports_history": True,
    "supports_passive_listen": False,
    "supports_card_edit": True,
    "card_edit_window_sec": 1209600,
    "inbound_file_in_group": True,
    "proactive_requires_prior_message": False,
    "outbound_rate_per_min": 60,
}

# §3.1 config.py 全默认时的整棵配置树，同样写死。
EXPECTED_AITE_CONFIG: dict[str, object] = {
    "tenant_id": "default",
    "platform": "feishu",
    "feishu": {
        "app_id_env": "FEISHU_APP_ID",
        "app_secret_env": "FEISHU_APP_SECRET",
        "bot_name": "Aite",
        "bot_open_id_env": "FEISHU_BOT_OPEN_ID",
        "history_window": 50,
    },
    "model": {
        "provider": "openai_compat",
        "base_url": "",
        "api_key_env": "AITE_MODEL_API_KEY",
        "model": "",
        "max_tokens": 4096,
        "temperature": 0.0,
        "price_in_per_mtok": 0.0,
        "price_out_per_mtok": 0.0,
    },
    "sandbox": {
        "image": "aite-sandbox:p0",
        "cpu": 1.0,
        "mem_mb": 1024,
        "idle_sec": 300,
        "exec_timeout_sec": 120,
    },
    "worker": {
        "max_steps": 40,
        "max_wall_sec": 1200,
        "card_update_min_interval_ms": 500,
        "system_prompt_path": "aite/worker/prompts/platform.md",
    },
    "storage": {
        "sqlite_path": "data/aite.db",
        "evidence_dir": "data/evidence",
        "artifacts_dir": "data/artifacts",
    },
}

# §3.1 config.py 各模型的字段名与声明顺序，同样写死。
EXPECTED_CONFIG_FIELDS: dict[str, tuple[str, ...]] = {
    "FeishuConfig": ("app_id_env", "app_secret_env", "bot_name", "bot_open_id_env", "history_window"),
    "ModelConfig": (
        "provider",
        "base_url",
        "api_key_env",
        "model",
        "max_tokens",
        "temperature",
        "price_in_per_mtok",
        "price_out_per_mtok",
    ),
    "SandboxConfig": ("image", "cpu", "mem_mb", "idle_sec", "exec_timeout_sec"),
    "WorkerConfig": ("max_steps", "max_wall_sec", "card_update_min_interval_ms", "system_prompt_path"),
    "StorageConfig": ("sqlite_path", "evidence_dir", "artifacts_dir"),
    "AiteConfig": ("tenant_id", "platform", "feishu", "model", "sandbox", "worker", "storage"),
}

_CONFIG_MODELS = (
    FeishuConfig,
    ModelConfig,
    SandboxConfig,
    WorkerConfig,
    StorageConfig,
    AiteConfig,
)

# 子模型名 -> 它在 EXPECTED_AITE_CONFIG 里对应的那棵子树的键。
_SUBTREE_KEY: dict[str, str] = {
    "FeishuConfig": "feishu",
    "ModelConfig": "model",
    "SandboxConfig": "sandbox",
    "WorkerConfig": "worker",
    "StorageConfig": "storage",
}

# 常见密钥前缀 + 「像随机串」的判据，用于确认配置里只有环境变量名、没有密钥本身。
_SECRET_PREFIXES = ("sk-", "sk_", "xoxb-", "xoxp-", "xapp-", "ghp_", "gho_", "AKIA", "AIza", "Bearer ")
_ENV_VAR_NAME = re.compile(r"^[A-Z][A-Z0-9_]*$")
_TOKENISH = re.compile(r"^[A-Za-z0-9+/=_-]{24,}$")


def _looks_like_secret(value: str) -> bool:
    """粗判一个字符串像不像"真密钥"：已知前缀，或 24+ 位的大小写数字混排随机串。"""
    if value.startswith(_SECRET_PREFIXES):
        return True
    return bool(
        _TOKENISH.match(value)
        and any(c.islower() for c in value)
        and any(c.isupper() for c in value)
        and any(c.isdigit() for c in value)
    )


def _walk_strings(node: object, path: str = "") -> list[tuple[str, str]]:
    """把嵌套 dict 里的所有字符串叶子摊平成 (点分路径, 值)。"""
    out: list[tuple[str, str]] = []
    if isinstance(node, dict):
        for key, sub in node.items():
            out.extend(_walk_strings(sub, f"{path}.{key}" if path else str(key)))
    elif isinstance(node, str):
        out.append((path, node))
    return out


def test_feishu_p0_is_platform_capabilities_instance() -> None:
    """§6 T0 验收：FEISHU_P0 可实例化，且类型就是 PlatformCapabilities。"""
    assert isinstance(FEISHU_P0, PlatformCapabilities)
    assert type(FEISHU_P0) is PlatformCapabilities


def test_feishu_p0_field_values_frozen_one_by_one() -> None:
    """逐字段写死 §3.1 的取值；改任一默认值都会红。"""
    assert FEISHU_P0.platform == "feishu"
    assert FEISHU_P0.supports_thread is True
    assert FEISHU_P0.supports_history is True
    assert FEISHU_P0.supports_passive_listen is False
    assert FEISHU_P0.supports_card_edit is True
    assert FEISHU_P0.card_edit_window_sec == 1209600
    assert FEISHU_P0.inbound_file_in_group is True
    assert FEISHU_P0.proactive_requires_prior_message is False
    assert FEISHU_P0.outbound_rate_per_min == 60


def test_feishu_p0_full_dump_matches_expected() -> None:
    """整份 model_dump() 比对：既冻结取值，也冻结「不多一个字段、不少一个字段」。"""
    assert FEISHU_P0.model_dump() == EXPECTED_FEISHU_P0


def test_platform_capabilities_field_names_and_order_frozen() -> None:
    """字段集合与声明顺序按 §3.1 原样冻结。"""
    assert tuple(PlatformCapabilities.model_fields) == (
        "platform",
        "supports_thread",
        "supports_history",
        "supports_passive_listen",
        "supports_card_edit",
        "card_edit_window_sec",
        "inbound_file_in_group",
        "proactive_requires_prior_message",
        "outbound_rate_per_min",
    )
    # 契约里九个字段全部必填，没有一个带默认值。
    assert all(field.is_required() for field in PlatformCapabilities.model_fields.values())


def test_card_edit_window_sec_is_fourteen_days_in_seconds() -> None:
    """1209600 秒 = 14 天，与 §3.1 注释「飞书 14 天」对齐。"""
    assert FEISHU_P0.card_edit_window_sec == 14 * 24 * 3600
    assert EXPECTED_FEISHU_P0["card_edit_window_sec"] == 14 * 24 * 3600


def test_platform_literal_members() -> None:
    """Platform 这个 Literal 只允许这四个值，顺序也照 §3.1。"""
    assert get_args(Platform) == ("feishu", "dingtalk", "wecom", "fake")
    assert FEISHU_P0.platform in get_args(Platform)


def test_invalid_platform_raises_validation_error() -> None:
    """platform="slack" 不在 Platform 里，pydantic 必须拒绝而不是放行。"""
    with pytest.raises(ValidationError) as excinfo:
        PlatformCapabilities(
            platform="slack",
            supports_thread=True,
            supports_history=True,
            supports_passive_listen=False,
            supports_card_edit=True,
            card_edit_window_sec=1209600,
            inbound_file_in_group=True,
            proactive_requires_prior_message=False,
            outbound_rate_per_min=60,
        )
    errors = excinfo.value.errors()
    assert len(errors) == 1
    assert errors[0]["loc"] == ("platform",)
    assert errors[0]["type"] == "literal_error"


def test_platform_capabilities_missing_fields_raise_validation_error() -> None:
    """九个字段全必填：空构造要报出九条 missing。"""
    with pytest.raises(ValidationError) as excinfo:
        PlatformCapabilities()
    missing = {err["loc"][0] for err in excinfo.value.errors() if err["type"] == "missing"}
    assert missing == set(EXPECTED_FEISHU_P0)


def test_platform_capabilities_json_round_trip() -> None:
    """§6 T0 验收的 JSON round-trip：dump 再 validate 回来必须逐字段相等。"""
    restored = PlatformCapabilities.model_validate_json(FEISHU_P0.model_dump_json())
    assert restored == FEISHU_P0
    assert restored.model_dump() == EXPECTED_FEISHU_P0


def test_aite_config_defaults_instantiable_and_key_values_match_spec() -> None:
    """AiteConfig() 不传任何参数就能建出来，关键几项按 §3.1 写死。"""
    cfg = AiteConfig()
    assert isinstance(cfg, AiteConfig)
    assert cfg.platform == "feishu"
    assert cfg.worker.max_steps == 40
    assert cfg.worker.max_wall_sec == 1200
    assert cfg.sandbox.idle_sec == 300
    assert cfg.storage.sqlite_path == "data/aite.db"
    assert cfg.model.api_key_env == "AITE_MODEL_API_KEY"


def test_aite_config_full_default_tree_frozen() -> None:
    """整份默认配置比对，防止有人悄悄改嵌套子模型的默认值。"""
    assert AiteConfig().model_dump() == EXPECTED_AITE_CONFIG


def test_aite_config_json_round_trip() -> None:
    """配置也走一遍 JSON round-trip。"""
    cfg = AiteConfig()
    restored = AiteConfig.model_validate_json(cfg.model_dump_json())
    assert restored == cfg
    assert restored.model_dump() == EXPECTED_AITE_CONFIG


def test_aite_config_env_fields_hold_env_var_names_not_secrets() -> None:
    """凡是 *_env 的字段，取值必须长得像环境变量名（全大写下划线）。"""
    leaves = _walk_strings(AiteConfig().model_dump())
    env_fields = {path: value for path, value in leaves if path.split(".")[-1].endswith("_env")}
    assert set(env_fields) == {
        "feishu.app_id_env",
        "feishu.app_secret_env",
        "feishu.bot_open_id_env",
        "model.api_key_env",
    }
    for path, value in env_fields.items():
        assert _ENV_VAR_NAME.match(value), f"{path} = {value!r} 不像环境变量名"


def test_aite_config_has_no_secret_looking_values() -> None:
    """整棵配置树扫一遍：不许出现 sk- / AKIA 这类前缀，也不许出现随机串状的长 token。"""
    offenders = [
        (path, value) for path, value in _walk_strings(AiteConfig().model_dump()) if _looks_like_secret(value)
    ]
    assert offenders == []
    # 反向自检：这个判据确实能认出密钥，不是恒为真的空断言。
    assert _looks_like_secret("sk-Ab3dEfGh1jKlMn0pQrStUvWx")
    assert not _looks_like_secret("FEISHU_APP_SECRET")


def test_aite_config_platform_literal_is_narrower_than_platform() -> None:
    """§3.1 里 AiteConfig.platform 只收 feishu/fake，比 Platform 窄；dingtalk 必须被拒。"""
    config_platforms = get_args(AiteConfig.model_fields["platform"].annotation)
    assert config_platforms == ("feishu", "fake")
    assert set(config_platforms) < set(get_args(Platform))
    with pytest.raises(ValidationError) as excinfo:
        AiteConfig(platform="dingtalk")
    errors = excinfo.value.errors()
    assert len(errors) == 1
    assert errors[0]["loc"] == ("platform",)
    assert errors[0]["type"] == "literal_error"


def test_model_config_provider_literal_members() -> None:
    """provider 这个 Literal 只允许 openai_compat / scripted，顺序照 §3.1；别的值要报错。"""
    assert get_args(ModelConfig.model_fields["provider"].annotation) == ("openai_compat", "scripted")
    assert ModelConfig().provider == "openai_compat"
    with pytest.raises(ValidationError) as excinfo:
        AiteConfig(model={"provider": "anthropic"})
    errors = excinfo.value.errors()
    assert len(errors) == 1
    assert errors[0]["loc"] == ("model", "provider")
    assert errors[0]["type"] == "literal_error"


def test_config_field_names_and_order_frozen() -> None:
    """六个配置模型的字段集合与声明顺序按 §3.1 原样冻结。"""
    for model in _CONFIG_MODELS:
        assert tuple(model.model_fields) == EXPECTED_CONFIG_FIELDS[model.__name__], model.__name__


def test_every_config_field_has_a_default() -> None:
    """config/aite.yaml 允许整段缺省：六个模型都不许出现必填字段。"""
    for model in _CONFIG_MODELS:
        required = [name for name, field in model.model_fields.items() if field.is_required()]
        assert required == [], f"{model.__name__} 出现必填字段 {required}"
        assert model().model_dump() == (
            EXPECTED_AITE_CONFIG if model is AiteConfig else EXPECTED_AITE_CONFIG[_SUBTREE_KEY[model.__name__]]
        )


def test_frozen_defaults_have_the_spec_declared_types() -> None:
    """`==` 认不出 1209600 与 1209600.0、True 与 1 的差别，这里把类型也钉死。"""
    dumped = FEISHU_P0.model_dump()
    assert type(dumped["platform"]) is str
    for name in (
        "supports_thread",
        "supports_history",
        "supports_passive_listen",
        "supports_card_edit",
        "inbound_file_in_group",
        "proactive_requires_prior_message",
    ):
        assert type(dumped[name]) is bool, name
    assert type(dumped["card_edit_window_sec"]) is int
    assert type(dumped["outbound_rate_per_min"]) is int

    cfg = AiteConfig().model_dump()
    for path in ("model.max_tokens", "sandbox.mem_mb", "sandbox.idle_sec", "sandbox.exec_timeout_sec",
                 "worker.max_steps", "worker.max_wall_sec", "worker.card_update_min_interval_ms",
                 "feishu.history_window"):
        section, field = path.split(".")
        assert type(cfg[section][field]) is int, path
    for path in ("sandbox.cpu", "model.temperature", "model.price_in_per_mtok", "model.price_out_per_mtok"):
        section, field = path.split(".")
        assert type(cfg[section][field]) is float, path
