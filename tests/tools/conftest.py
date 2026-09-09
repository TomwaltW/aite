"""scripts/ 不是包，测试要用 evidence_show 就得先把它放到 sys.path 上。"""
import sys
from pathlib import Path

_SCRIPTS = Path(__file__).resolve().parents[2] / "scripts"
if str(_SCRIPTS) not in sys.path:
    sys.path.insert(0, str(_SCRIPTS))
