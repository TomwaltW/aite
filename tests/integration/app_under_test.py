"""被测组装的唯一入口 —— 整个 tests/integration 只从这里取件。

    from aite.app import AiteApp, build_app, run_app

并轨已经做完（T11）：T8 那个 227 行的最小替身 `t8_minimal_app.py` 已删除，
这里指的就是生产组装 `aite/app.py`。**不要再把它指回任何替身** ——
这一层测的是进程级接线，指回替身就等于这一整目录的测试全部空转。

tests/integration/ 下的每个测试都只从这里拿 `AiteApp` / `build_app` / `run_app`，
所以换掉这一行就换掉了整个被测组装。三个名字与签名由冻结契约 C-TΩ-1 保证。
"""
from aite.app import AiteApp, build_app, run_app

__all__ = ["AiteApp", "build_app", "run_app"]
