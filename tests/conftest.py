"""全仓 pytest 引导（T0 owner，§3.4）。

只做一件事：把**本仓库根**顶到 sys.path 最前面。

为什么必须做：T1–T4 各自在 worktree 里干活，而 §6 只有 T0 的验收含 A1
（`pip install -e`）。别的轨如果沿用主仓那个 editable 安装直接跑 `pytest -q`，
`import aite` 会解析到主仓的 /Users/.../Aite/aite，测的是别人的代码 —— 全绿也毫无意义。
pytest 的 prepend 导入模式只会把**测试文件所在目录**塞进 sys.path，兜不住这一层。

各 track 的 fixture 放自己目录的 conftest.py，不要动这个文件（§3.4）。
"""
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

if sys.path[:1] != [str(REPO_ROOT)]:
    sys.path.insert(0, str(REPO_ROOT))
