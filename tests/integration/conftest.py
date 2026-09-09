"""tests/integration 的 fixture（owner: T8，§3.4：各 track 的 fixture 放自己目录）。

这一层测的是**进程级接线**：SQLite 真落盘、evidence 真写文件、`run_app` 真收尾。
所以这里刻意不提供任何「跳过组装直接拿 plane / worker」的捷径 —— 每个用例都必须
从 `build_app` 走一遍，否则测的就不是接线了。

被测组装从哪来：`app_under_test.py` 一行 import。并轨时只改那一行。
"""
from pathlib import Path

import pytest
from integration_fakes import make_config

from aite.contracts import AiteConfig


@pytest.fixture
def config(tmp_path: Path) -> AiteConfig:
    """全部落盘路径都在 tmp_path 下。仓库里的 data/ 一个字节都不许写。"""
    return make_config(tmp_path)
