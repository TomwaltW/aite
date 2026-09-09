"""T1 自己的 pytest fixture（§3.4）。

按 §3.4 的「测试替身规则」，这里**不 import `aite/testing/`**（那是 T4 的，并行期间
还是空包）。假时钟、假连接都各自自建 —— 重复远比冲突便宜。

`tests/conftest.py` 是 T0 的（worktree 的 sys.path 引导），一个字都不动。
"""
from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest

from aite.contracts import ChecklistCard, ChecklistItemView

# 与 tests/fixtures/feishu/*.json 里写死的是同一套 id。
BOT_OPEN_ID = "ou_aite_bot_0000000000000000000001"
APP_ID = "cli_a1b2c3d4e5f60123"
CHAT_ID = "oc_chat_p0_demo_0001"
ROOT_MESSAGE_ID = "om_toplevel_0001"

FIXTURES_DIR = Path(__file__).resolve().parent.parent / "fixtures" / "feishu"


class FakeClock:
    """手动推进的单调时钟；`sleep` 只记账、不真睡。"""

    def __init__(self, start: float = 0.0) -> None:
        self.now = start
        self.slept: list[float] = []

    def __call__(self) -> float:
        return self.now

    def advance(self, seconds: float) -> None:
        self.now += seconds

    async def sleep(self, seconds: float) -> None:
        self.slept.append(seconds)
        self.now += seconds


@pytest.fixture
def fixtures_dir() -> Path:
    return FIXTURES_DIR


@pytest.fixture
def fake_clock() -> FakeClock:
    return FakeClock()


@pytest.fixture
def bot_open_id() -> str:
    return BOT_OPEN_ID


def sample_card(**overrides: Any) -> ChecklistCard:
    """一张典型的进行中卡片。"""
    payload: dict[str, Any] = {
        "task_id": "b7c1e6f0-1111-4222-8333-444455556666",
        "task_no": "#A17",
        "title": "把 Q3 销售数据画成趋势图",
        "initiator": "张三",
        "started_at": "9:02",
        "status": "working",
        "items": [
            ChecklistItemView(id="c1", text="读取 CSV", state="done"),
            ChecklistItemView(id="c2", text="按月汇总", state="doing", note="共 3 个 sheet"),
            ChecklistItemView(id="c3", text="出图并回传", state="todo"),
        ],
        "footer": "预计 2 分钟 · 已用 ¥0.12",
        "actions": ["stop"],
    }
    payload.update(overrides)
    return ChecklistCard(**payload)


@pytest.fixture
def checklist_card() -> ChecklistCard:
    return sample_card()
