# aite/contracts/config.py  —— config/aite.yaml 的形状。密钥只放环境变量名，绝不放值。
from typing import Literal

from pydantic import BaseModel, Field


class FeishuConfig(BaseModel):
    app_id_env: str = "FEISHU_APP_ID"
    app_secret_env: str = "FEISHU_APP_SECRET"
    bot_name: str = "Aite"
    bot_open_id_env: str = "FEISHU_BOT_OPEN_ID"   # 用于识别 @ 的是不是自己
    history_window: int = 50

class ModelConfig(BaseModel):
    provider: Literal["openai_compat", "scripted"] = "openai_compat"
    base_url: str = ""
    api_key_env: str = "AITE_MODEL_API_KEY"
    model: str = ""
    max_tokens: int = 4096
    temperature: float = 0.0
    price_in_per_mtok: float = 0.0       # 元/百万 token，只用于卡片上的"已用 ¥"
    price_out_per_mtok: float = 0.0

class SandboxConfig(BaseModel):
    image: str = "aite-sandbox:p0"
    cpu: float = 1.0
    mem_mb: int = 1024
    idle_sec: int = 300
    exec_timeout_sec: int = 120

class WorkerConfig(BaseModel):
    max_steps: int = 40
    max_wall_sec: int = 1200
    card_update_min_interval_ms: int = 500
    system_prompt_path: str = "aite/worker/prompts/platform.md"

class StorageConfig(BaseModel):
    sqlite_path: str = "data/aite.db"
    evidence_dir: str = "data/evidence"
    artifacts_dir: str = "data/artifacts"

class AiteConfig(BaseModel):
    tenant_id: str = "default"
    platform: Literal["feishu", "fake"] = "feishu"
    feishu: FeishuConfig = Field(default_factory=FeishuConfig)
    model: ModelConfig = Field(default_factory=ModelConfig)
    sandbox: SandboxConfig = Field(default_factory=SandboxConfig)
    worker: WorkerConfig = Field(default_factory=WorkerConfig)
    storage: StorageConfig = Field(default_factory=StorageConfig)
