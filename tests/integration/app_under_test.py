"""被测组装的唯一入口 —— 并轨时**只改下面这一行**。

    并轨前（T7 的 aite/app.py 还没交）：
        from t8_minimal_app import AiteApp, build_app, run_app

    并轨后：
        from aite.app import AiteApp, build_app, run_app
    并把 tests/integration/t8_minimal_app.py 删掉。

tests/integration/ 下的每个测试都只从这里拿 `AiteApp` / `build_app` / `run_app`，
所以换掉这一行就换掉了整个被测组装。三个名字与签名由冻结契约 C-TΩ-1 保证。
"""
from t8_minimal_app import AiteApp, build_app, run_app

__all__ = ["AiteApp", "build_app", "run_app"]
