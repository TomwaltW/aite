"""冻结契约 C-TΩ-1 的组装面本身。

前四组测的是「接起来之后跑得对不对」，这一组测的是**接口没走样**：字段名、
三个替身口子、`run_app` 的关键字参数与那个 20.0 的默认值、以及硬约束 1
「`build_app` 只组装，不产生副作用」。

并轨那天先跑这一个文件：它红了就说明 T7 的 `aite/app.py` 与 C-TΩ-1 对不上，
后面三组的红都是它的连锁反应，不用一条条看。
"""
import inspect
from dataclasses import fields, is_dataclass

from app_under_test import AiteApp, build_app, run_app
from integration_fakes import GatedPlatform, RecordingModel, final_step

from aite.testing import FakeSandbox

#: C-TΩ-1 里 AiteApp 逐字写死的字段
REQUIRED_FIELDS = (
    "config",
    "platform",
    "store",
    "evidence",
    "sandbox",
    "gateway",
    "model",
    "worker",
    "plane",
    "ingress",
)


def test_aite_app_has_the_frozen_fields():
    assert is_dataclass(AiteApp)
    names = {f.name for f in fields(AiteApp)}
    missing = [f for f in REQUIRED_FIELDS if f not in names]
    assert not missing, f"AiteApp 少了 C-TΩ-1 写死的字段：{missing}"


def test_build_app_keeps_the_three_injection_points():
    sig = inspect.signature(build_app)
    for name in ("platform", "model", "sandbox"):
        param = sig.parameters.get(name)
        assert param is not None, f"build_app 少了 {name}= 这个口子"
        assert param.kind is inspect.Parameter.KEYWORD_ONLY, f"{name} 必须是关键字参数"
        assert param.default is None, f"{name} 不传时才按 config 造真的，默认值应当是 None"


def test_run_app_grace_period_defaults_to_twenty_seconds():
    """对齐 docker-compose.yml 的 `stop_grace_period: 20s`。"""
    sig = inspect.signature(run_app)
    stop = sig.parameters.get("stop")
    assert stop is not None and stop.kind is inspect.Parameter.KEYWORD_ONLY
    assert stop.default is None

    grace = sig.parameters.get("shutdown_grace_sec")
    assert grace is not None, "宽限期必须做成 run_app 的关键字参数（C-TΩ-1）"
    assert grace.kind is inspect.Parameter.KEYWORD_ONLY
    assert grace.default == 20.0


def test_build_app_has_no_side_effects(config):
    """硬约束 1：只组装。不连网、不起容器、不发消息，也不需要事件循环。

    这个用例是**同步**的（不是 `async def`）—— 没有 running loop 时 `build_app`
    照样得能跑完。真进程里它就是在 `asyncio.run` 之前被调用的。
    """
    platform = GatedPlatform()
    model = RecordingModel([final_step("没人会叫我。")])
    sandbox = FakeSandbox()

    app = build_app(config, platform=platform, model=model, sandbox=sandbox)

    # 给了就用给的
    assert app.config is config
    assert app.platform is platform
    assert app.model is model
    assert app.sandbox is sandbox

    # 该装的都装上了
    for name in REQUIRED_FIELDS:
        assert getattr(app, name) is not None, f"AiteApp.{name} 没装上"

    # 一个副作用都没有
    assert len(platform.calls) == 0, f"build_app 碰了平台：{platform.calls.methods()}"
    assert len(sandbox.calls) == 0, f"build_app 碰了沙箱：{sandbox.calls.methods()}"
    assert model.call_count == 0
    assert platform.started is False and platform.stopped is False
