"""冻结契约 C-TΩ-1 的一份**一次性**最小实现（owner: T8）。

⚠️ 这个文件在并轨时会被整份丢掉。T7 交出 `aite/app.py` 之后，只需要把
`app_under_test.py` 里那一行 import 换掉，本文件删除即可 —— 集成测试一条都不用改。

为什么要有它：T8 与 T7 并行，T8 开工时 `aite/app.py` 还是 T0 留下的 stub。
照 C-TΩ-1 自己写一份最小组装，这一轨就不用等任何人。

自己给自己的三条纪律（并轨后测试还绿不绿全看这三条）：

1. 只实现 C-TΩ-1 写死的面：`AiteApp` 的字段名、`build_app` 的三个关键字口子、
   `run_app(app, *, stop=..., shutdown_grace_sec=20.0)`、以及那段退出序列。
2. 不往外暴露任何本文件独有的辅助面 —— 测试拿不到就不会依赖。
3. 组装用的全是 `aite/` 下的真实现（SqliteSessionStore / FileEvidenceWriter /
   P0ToolGateway / InProcessControlPlane / AgentWorker / Ingress），
   只有平台、模型、沙箱三个口子留给测试塞替身。
"""
from __future__ import annotations

import asyncio
import contextlib
from dataclasses import dataclass
from typing import Any

from aite.contracts import (
    AiteConfig,
    EvidenceWriter,
    ModelPort,
    PlatformPort,
    SandboxPort,
    SandboxSpec,
    Session,
    SessionStore,
    Task,
    ToolGateway,
)
from aite.control import InProcessControlPlane, SqliteSessionStore
from aite.evidence import FileEvidenceWriter
from aite.gateway import P0ToolGateway
from aite.ingress import Ingress
from aite.worker import AgentWorker

#: C-TΩ-1：对齐 docker-compose.yml 的 stop_grace_period: 20s
SHUTDOWN_GRACE_SEC = 20.0


@dataclass
class AiteApp:
    """C-TΩ-1 的组装面。字段名逐字照抄，测试只准看这些。"""

    config: AiteConfig
    platform: PlatformPort
    store: SessionStore
    evidence: EvidenceWriter
    sandbox: SandboxPort | None
    gateway: ToolGateway | None
    model: ModelPort
    worker: AgentWorker
    plane: Any            # InProcessControlPlane
    ingress: Ingress


class _TokenRegisteringWorker(AgentWorker):
    """任务开跑前，把 `Task.session_token` 登记给 Gateway。

    这一段是 **TΩ 必须补的接线**，不是本文件的花活：`P0ToolGateway` 的
    `_check_token` 是失败关闭的，没登记过的 task 调任何 Gateway 工具一律 `denied`；
    而 `aite/` 下今天没有任何地方调 `register_task`，也没有谁传 `token_resolver`
    （`token_resolver` 是同步签名，接不上异步的 `SessionStore.get_task`）。
    不补这一段，`run_python` / `read_document` 这些工具在真进程里全是 denied。

    挂在 worker 的 `run()` 上而不是别处，是因为这里是「任务真要开跑」与
    「拿得到 session_token」同时成立的第一个公开点。
    """

    def __init__(self, *, gateway: ToolGateway | None = None, **kwargs: Any) -> None:
        super().__init__(gateway=gateway, **kwargs)
        self._gateway_for_token = gateway

    async def run(self, task: Task, session: Session, **kwargs: Any) -> Task:
        register = getattr(self._gateway_for_token, "register_task", None)
        if callable(register) and task.session_token:
            register(task.id, task.session_token)
        return await super().run(task, session, **kwargs)


def build_app(
    config: AiteConfig,
    *,
    platform: PlatformPort | None = None,
    model: ModelPort | None = None,
    sandbox: SandboxPort | None = None,
) -> AiteApp:
    """只组装，不产生副作用：不连网、不起容器、不发消息、不碰事件循环。

    SQLite 与 evidence 目录这里也不建 —— `SqliteSessionStore.__init__` 与
    `FileEvidenceWriter.__init__` 都只记路径，真正的建表建目录在 `run_app`
    的 `plane.init()` / 第一次 append 时发生。C-TΩ-1 允许在这里建，
    但「能不做就不做」更省事，也更好证明这条约束成立。
    """
    store = SqliteSessionStore(config.storage.sqlite_path)
    evidence = FileEvidenceWriter(config.storage.evidence_dir)

    platform = platform if platform is not None else _real_platform(config)
    model = model if model is not None else _real_model(config)
    sandbox = sandbox if sandbox is not None else _real_sandbox(config)

    sc = config.sandbox
    gateway = P0ToolGateway(
        platform=platform,
        sandbox=sandbox,
        sandbox_spec=SandboxSpec(image=sc.image, cpu=sc.cpu, mem_mb=sc.mem_mb),
    )
    worker = _TokenRegisteringWorker(
        store=store,
        platform=platform,
        model=model,
        evidence=evidence,
        config=config,
        gateway=gateway,
        sandbox=sandbox,
    )
    plane = InProcessControlPlane(
        store=store,
        platform=platform,
        evidence=evidence,
        config=config,
        model=model,
        gateway=gateway,
        sandbox=sandbox,
        worker=worker,
    )
    return AiteApp(
        config=config,
        platform=platform,
        store=store,
        evidence=evidence,
        sandbox=sandbox,
        gateway=gateway,
        model=model,
        worker=worker,
        plane=plane,
        ingress=Ingress(plane),
    )


async def run_app(
    app: AiteApp,
    *,
    stop: asyncio.Event | None = None,
    shutdown_grace_sec: float = SHUTDOWN_GRACE_SEC,
) -> None:
    """起飞 → 等 stop → 按 C-TΩ-1 写死的顺序收尾。"""
    await app.plane.init()                       # 建表；落盘路径的必要准备
    stop = stop if stop is not None else asyncio.Event()

    await app.platform.start(app.ingress.on_event)
    loop_task = asyncio.create_task(app.plane.run_forever())
    try:
        await stop.wait()
    finally:
        await _shutdown(app, loop_task, shutdown_grace_sec)


async def _shutdown(app: AiteApp, loop_task: asyncio.Task, grace: float) -> None:
    await app.platform.stop()                    # 先闭嘴，不再收新事件

    try:
        await asyncio.wait_for(app.plane.join(), grace)   # 在跑/排队的任务收尾
    except TimeoutError:
        pass                                     # 收不完就往下走，取消它

    # 超时时这一步是 C-TΩ-1 要求的取消；没超时时它只是在 queue.get() 上空转，
    # 不取消就会留成一条挂着的 task（连带 W7 的 reaper），所以两条路都收掉。
    loop_task.cancel()
    with contextlib.suppress(asyncio.CancelledError, Exception):
        await loop_task

    if app.sandbox is not None:
        # SandboxPort（§3.2 冻结）里**没有** aclose，只有 DockerSandbox 有。
        # 直接裸调会在任何非 Docker 沙箱上炸掉，所以按「有才调」处理。
        aclose = getattr(app.sandbox, "aclose", None)
        if callable(aclose):
            with contextlib.suppress(Exception):
                await aclose()

    close = getattr(app.store, "close", None)    # SessionStore 协议里也没有 close
    if callable(close):
        await close()


def main() -> None:
    """C-TΩ-1 硬约束 3 归 T7（人话报错 + 非零退出码）。

    本最小组装不承担进程入口，集成测试也不测它 —— 测了就成了测 T8 自己写的文案，
    并轨后必红。起飞前自检那一面归 T9。
    """
    raise NotImplementedError("进程入口由 T7 的 aite.app.main 实现；这里是 T8 的一次性组装")


# --------------------------------------------------------------------------
# 「不给就按 config 造真的」那三条口子
# --------------------------------------------------------------------------
#
# C-TΩ-1 要求不给就按 config 造真的（飞书长连接 / OpenAI-compat 客户端 / DockerSandbox）。
# 那是 T7 的活：造真的意味着 build_app 要 import lark-oapi 与 docker SDK，
# 而这一轨的测试永远从三个口子塞替身进来，一次都走不到这里。
# 与其照猜 T1/T3 的构造函数写一份会过期的代码，不如把话说明白。


def _unsupported(what: str) -> Any:
    raise NotImplementedError(
        f"T8 的最小组装不造真的{what}：请通过 build_app(..., platform=/model=/sandbox=) 传入。"
        "真实构造归 T7 的 aite/app.py。"
    )


def _real_platform(config: AiteConfig) -> PlatformPort:
    return _unsupported(f"平台（config.platform={config.platform}）")


def _real_model(config: AiteConfig) -> ModelPort:
    return _unsupported(f"模型（config.model.provider={config.model.provider}）")


def _real_sandbox(config: AiteConfig) -> SandboxPort:
    return _unsupported(f"沙箱（config.sandbox.image={config.sandbox.image}）")
