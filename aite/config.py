"""读 config/aite.yaml -> AiteConfig（脚手架，T0 写完）。

密钥不在这里出现：契约里存的是环境变量名（如 FEISHU_APP_SECRET），
取值由各 track 在用到的时候自己从 os.environ 读。
"""
from pathlib import Path

import yaml

from .contracts import AiteConfig

DEFAULT_CONFIG_PATH = Path("config/aite.yaml")


def load_config(path: str | Path = DEFAULT_CONFIG_PATH) -> AiteConfig:
    """把 YAML 配置加载成 AiteConfig。文件缺字段就用契约默认值；空文件 = 全默认。"""
    p = Path(path)
    if not p.exists():
        raise FileNotFoundError(f"配置文件不存在：{p}（可从 config/aite.example.yaml 复制）")
    data = yaml.safe_load(p.read_text(encoding="utf-8"))
    if data is None:
        data = {}
    if not isinstance(data, dict):
        raise ValueError(f"配置文件顶层必须是 mapping，实际是 {type(data).__name__}：{p}")
    return AiteConfig.model_validate(data)
