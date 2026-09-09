"""进程入口 —— 把 §3.2 的九个零件装成一个能起飞的常驻进程（owner: TΩ，§3.4 归属表 / §5 任务表）。

三层，一层比一层多做一点事：

* `build_app` **只组装，不产生副作用**：不连网、不起容器、不发消息。唯一落盘的是按
  `StorageConfig` 建 SQLite 的父目录与 evidence 目录 —— 那是落盘路径的必要准备。
  `platform` / `model` / `sandbox` 三个口子给了就用给的，不给才按 config 造真的；
  集成测试靠它们把替身塞进来测真实接线。
* `run_app` 起长连接、把派发循环挂后台、等停机信号，然后按写死的顺序收尾。
* `main` 命令行 + 起飞前体检。配置读不到、凭证没设、`base_url` / `model` 是空串这类
  一眼能看出来的问题，一律一行中文说清缺什么、怎么补，然后非零退出码；
  裸 traceback 只在 `--traceback` 时打 stderr。

组装这一层自己扛的两件接线写在 `AppWorker` 的 docstring 里 —— 它们是「只有组装方
才知道」的东西，不是谁的实现漏了。
"""
from __future__ import annotations

import argparse
import asyncio
import contextlib
import logging
import os
import signal
import sys
import traceback
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .config import DEFAULT_CONFIG_PATH, load_config
from .contracts import (
    AiteConfig,
    EvidenceWriter,
    ModelPort,
    PlatformPort,
    SandboxPort,
    SandboxSpec,
    Session,
    SessionStore,
    StorageConfig,
    Task,
    ToolGateway,
)
from .control.plane import InProcessControlPlane
from .control.store import SqliteSessionStore
from .evidence.writer import FileEvidenceWriter
from .gateway.tool_gateway import P0ToolGateway
from .ingress.handler import Ingress
from .worker.context import load_system_prompt
from .worker.loop import AgentWorker

log = logging.getLogger("aite.app")

#: 优雅退出的宽限期，对齐 `docker-compose.yml` 的 `stop_grace_period: 20s`。
DEFAULT_SHUTDOWN_GRACE_SEC = 20.0

#: 起飞前体检没过 / 配置读不到时的退出码。跟评测 runner 的「起不来」一个口径。
EXIT_STARTUP = 2
#: 第二次收到信号硬退时的退出码。
EXIT_HARD_STOP = 130


class StartupError(RuntimeError):
    """起飞前就能看出来的问题。消息本身就是给人看的那句话，`main` 直接原样打出去。"""


# --------------------------------------------------------------------------
# 组装这一层自己扛的接线
# --------------------------------------------------------------------------

class AppWorker(AgentWorker):
    """真机组装用的 `AgentWorker`。在 T2 的实现之上补两件只有组装方知道的事。

    **一、给 Gateway 登记 `session_token`。**
    `P0ToolGateway` 的令牌校验是失败关闭的（见 `aite/gateway/tool_gateway.py` 的模块
    docstring：`register_task()` 现场登记，或构造时给 `token_resolver`，两个都没有 =
    谁也别想调工具）。而 `Task.session_token` 是 ControlPlane 建任务时才生成的，
    `SessionStore.get_task` 又是 async 的、`TokenResolver` 却是同步签名 ——
    两头唯一接得上的位置就是任务真正开跑的这一刻。不接的话真机上每个工具调用都是
    `denied`，M3 / M4 一条都过不去。释放走 worker 与控制面已有的 `release_task`，
    这里只管登记。

    **二、记住在飞的任务。**
    宽限期超时后要给它们收场（写 evidence、把卡片置 cancelled），而 §3.2 的
    `SessionStore` 没有「列出全部活跃任务」的口子（`list_active_tasks` 要 chat_id），
    控制面也不对外暴露在跑的任务集合。`run()` 正好两头都看得见，就在这里记一笔。
    """

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        #: task_id -> (task, session)，只在任务真正跑着的那段时间里有值。
        self.in_flight: dict[str, tuple[Task, Session]] = {}

    async def run(self, task: Task, session: Session, **kwargs: Any) -> Task:
        register = getattr(self._gateway, "register_task", None)
        if callable(register):
            register(task.id, task.session_token)
        self.in_flight[task.id] = (task, session)
        try:
            return await super().run(task, session, **kwargs)
        finally:
            self.in_flight.pop(task.id, None)


# --------------------------------------------------------------------------
# 冻结契约 C-TΩ-1：app 组装面
# --------------------------------------------------------------------------

@dataclass
class AiteApp:
    """装好的一整套。字段顺序与名字由 C-TΩ-1 冻结，集成测试按名字取件。"""

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


def build_app(
    config: AiteConfig,
    *,
    platform: PlatformPort | None = None,
    model: ModelPort | None = None,
    sandbox: SandboxPort | None = None,
) -> AiteApp:
    """按 config 把零件装起来。**只组装**：不连网、不起容器、不发消息。

    三个口子给了就用给的，不给才按 config 造真的。造真的这一路顺带做起飞前体检 ——
    凭证、端点、模型名缺了就地抛 `StartupError`，`main` 把它打成一行人话。
    注入进来的那一路一个字都不查：替身不需要凭证。
    """
    _prepare_storage(config.storage)
    _require_system_prompt(config)

    plat = platform if platform is not None else _build_platform(config)
    mdl = model if model is not None else _build_model(config)
    box = sandbox if sandbox is not None else _build_sandbox()

    store = SqliteSessionStore(config.storage.sqlite_path)
    evidence = FileEvidenceWriter(config.storage.evidence_dir)
    gateway = P0ToolGateway(platform=plat, sandbox=box, sandbox_spec=_sandbox_spec(config))
    worker = AppWorker(
        store=store,
        platform=plat,
        model=mdl,
        evidence=evidence,
        config=config,
        gateway=gateway,
        sandbox=box,
    )
    plane = InProcessControlPlane(
        store=store,
        platform=plat,
        evidence=evidence,
        config=config,
        model=mdl,
        gateway=gateway,
        sandbox=box,
        worker=worker,
    )
    return AiteApp(
        config=config,
        platform=plat,
        store=store,
        evidence=evidence,
        sandbox=box,
        gateway=gateway,
        model=mdl,
        worker=worker,
        plane=plane,
        ingress=Ingress(plane),
    )


async def run_app(
    app: AiteApp,
    *,
    stop: asyncio.Event | None = None,
    shutdown_grace_sec: float = DEFAULT_SHUTDOWN_GRACE_SEC,
) -> None:
    """起飞，然后一直跑到 SIGINT / SIGTERM 或调用方把 `stop` 置起来。

    退出序列由 C-TΩ-1 写死，一步都不跳：

        platform.stop()                        先闭嘴，不再收新事件
        plane.join() 限时 shutdown_grace_sec    在跑 / 排队的任务收尾
        超时 -> 取消 run_forever 那条 task
        sandbox.aclose()（有沙箱才调）
        store.close()
    """
    stop_event = stop if stop is not None else asyncio.Event()
    loop = asyncio.get_running_loop()
    detach = _install_signal_handlers(loop, stop_event)

    # 建表。事件在 `platform.start` 返回的下一刻就可能到，表得先在（§3.2 init 幂等）。
    await app.store.init()
    _log_takeoff(app)

    runner: asyncio.Task | None = None
    try:
        await app.platform.start(app.ingress.on_event)
        runner = asyncio.ensure_future(app.plane.run_forever())
        await _serve(stop_event, runner)
    finally:
        # handler 一直挂到收尾做完才摘：人再按一次 Ctrl-C 基本都是**因为**收尾在磨蹭，
        # 提前摘掉的话第二次信号就落回 Python 默认处理，硬退这条路等于没有。
        try:
            await _shutdown(app, runner, grace=shutdown_grace_sec)
        finally:
            detach()


def main() -> None:
    """`python -m aite.app`。C-TΩ-1 把它冻结成零参数，实际工作在 `_main` 里。"""
    sys.exit(_main(sys.argv[1:]))


# --------------------------------------------------------------------------
# 造零件（只在没注入时走）
# --------------------------------------------------------------------------

def _prepare_storage(cfg: StorageConfig) -> None:
    """建落盘目录。C-TΩ-1 硬约束 1 允许的唯一副作用。"""
    if cfg.sqlite_path != ":memory:":
        Path(cfg.sqlite_path).parent.mkdir(parents=True, exist_ok=True)
    Path(cfg.evidence_dir).mkdir(parents=True, exist_ok=True)


def _require_system_prompt(config: AiteConfig) -> None:
    """W9 那四条铁律在 `platform.md` 里。缺了的话每个任务都会在第一步炸，起飞前就说。"""
    try:
        load_system_prompt(config.worker.system_prompt_path)
    except OSError as exc:
        raise StartupError(
            f"读不到 system prompt：{exc}。"
            f"配置项是 worker.system_prompt_path（当前值 {config.worker.system_prompt_path}），"
            "路径相对于进程的工作目录 —— 多半是没在仓库根起进程。"
        ) from exc


def _build_platform(config: AiteConfig) -> PlatformPort:
    if config.platform == "fake":
        raise StartupError(
            "config.platform=fake 时必须由调用方注入平台实现（build_app(platform=...)）。"
            "fake 是留给评测 / 回放的取值（§3.1），不会去连真实飞书；"
            "真机起飞请把 config/aite.yaml 的 platform 改成 feishu。"
        )

    cfg = config.feishu
    missing = [
        name
        for name in (cfg.app_id_env, cfg.app_secret_env, cfg.bot_open_id_env)
        if not (os.environ.get(name) or "").strip()
    ]
    if missing:
        raise StartupError(
            f"飞书凭证没设：环境变量 {'、'.join(missing)} 是空的。先 export 它们再起。"
            "（变量名由 config/aite.yaml 的 feishu.*_env 决定；密钥只走环境变量，别写进配置文件。）"
        )

    # 延迟 import：platform=fake 的调用方不必把 adapter 一起拖起来。
    from .adapters.feishu import FeishuPlatform

    return FeishuPlatform.from_config(cfg, tenant_id=config.tenant_id)


def _build_model(config: AiteConfig) -> ModelPort:
    cfg = config.model
    if cfg.provider != "openai_compat":
        raise StartupError(
            f"config.model.provider={cfg.provider} 时必须由调用方注入模型实现"
            "（build_app(model=...)）。scripted 是留给评测的取值（§3.1）。"
        )

    blanks = []
    if not cfg.base_url.strip():
        blanks.append("model.base_url（百炼 / 智谱这类 OpenAI 兼容端点）")
    if not cfg.model.strip():
        blanks.append("model.model（要用的模型名）")
    if blanks:
        raise StartupError(f"config/aite.yaml 里 {'、'.join(blanks)} 还是空串，填上再起。")

    from .models.openai_compat import ModelConfigError, OpenAICompatModel, resolve_api_key

    try:
        resolve_api_key(cfg)          # 只验有没有，取值不留在这儿
    except ModelConfigError as exc:
        raise StartupError(str(exc)) from exc
    return OpenAICompatModel(cfg)


def _build_sandbox() -> SandboxPort:
    # 全默认即可；docker 客户端是首次 acquire 时才建的，这里不碰 daemon。
    from .sandbox.docker_sandbox import DockerSandbox

    return DockerSandbox()


def _sandbox_spec(config: AiteConfig) -> SandboxSpec:
    cfg = config.sandbox
    return SandboxSpec(image=cfg.image, cpu=cfg.cpu, mem_mb=cfg.mem_mb)


# --------------------------------------------------------------------------
# 起飞 / 停机
# --------------------------------------------------------------------------

def _log_takeoff(app: AiteApp) -> None:
    """一行「接了谁」。真机排障时这是第一现场，别省。"""
    log.info(
        "aite.up platform=%s/%s model=%s sandbox=%s sqlite=%s evidence=%s",
        app.config.platform,
        type(app.platform).__name__,
        getattr(app.model, "name", "") or "(未命名)",
        app.config.sandbox.image if app.sandbox is not None else "(无沙箱)",
        app.config.storage.sqlite_path,
        app.config.storage.evidence_dir,
    )


def _install_signal_handlers(loop: asyncio.AbstractEventLoop, stop_event: asyncio.Event):
    """SIGINT / SIGTERM 都进同一条优雅退出路径；第二次立刻硬退。

    返回一个把 handler 摘掉的函数 —— 收尾阶段不该再被信号改道。非主线程里
    （集成测试常这么跑）根本挂不上，那时静默降级成「只认调用方传进来的 Event」。
    """
    hits = 0
    installed: list[signal.Signals] = []

    def _on_signal(sig: signal.Signals) -> None:
        nonlocal hits
        hits += 1
        if hits == 1:
            log.warning("aite.signal %s 收到，开始优雅退出（再来一次立即硬退）", sig.name)
            stop_event.set()
            return
        # 别让人 Ctrl-C 按不动：日志可能还压在 handler 的缓冲里，直接写 stderr。
        print(f"aite: 又收到 {sig.name}，硬退出。", file=sys.stderr, flush=True)
        os._exit(EXIT_HARD_STOP)

    for sig in (signal.SIGINT, signal.SIGTERM):
        try:
            loop.add_signal_handler(sig, _on_signal, sig)
        except (NotImplementedError, RuntimeError, ValueError):
            log.debug("aite.signal_unavailable %s（非主线程或平台不支持）", sig.name)
            continue
        installed.append(sig)

    def detach() -> None:
        for sig in installed:
            with contextlib.suppress(Exception):
                loop.remove_signal_handler(sig)

    return detach


async def _serve(stop_event: asyncio.Event, runner: asyncio.Task) -> None:
    """等停机信号。派发循环先一步死掉的话也别干等 —— 那时该进收尾。"""
    waiter = asyncio.ensure_future(stop_event.wait())
    try:
        done, _ = await asyncio.wait({waiter, runner}, return_when=asyncio.FIRST_COMPLETED)
    finally:
        waiter.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await waiter
    if runner in done and not runner.cancelled() and runner.exception() is not None:
        log.error("aite.plane_died 派发循环异常收场，进入收尾", exc_info=runner.exception())


async def _shutdown(app: AiteApp, runner: asyncio.Task | None, *, grace: float) -> None:
    """C-TΩ-1 硬约束 2 的退出序列。每一步都包着 suppress：收尾不许被单点失败带偏。"""
    log.info("aite.stopping grace=%.1fs pending=%d", grace, _pending(app))

    with contextlib.suppress(Exception):
        await app.platform.stop()

    stranded: list[tuple[Task, Session]] = []
    if runner is not None and not runner.done():
        try:
            await asyncio.wait_for(app.plane.join(), timeout=grace)
        except TimeoutError:
            # 超时这一支：先把在飞的任务抄下来，取消之后就再也问不出它们是谁了。
            stranded = list(getattr(app.worker, "in_flight", {}).values())
            log.warning(
                "aite.shutdown_timeout %.1fs 内没收完：在跑 %d 个、排队 %d 个，强行取消",
                grace,
                len(stranded),
                _pending(app),
            )
        except Exception:
            log.exception("aite.join_failed")

    if runner is not None:
        runner.cancel()
        with contextlib.suppress(asyncio.CancelledError, Exception):
            await runner

    # 被硬取消的任务自己没机会收场（worker 的 cancel 分支只在每步开头查标志位，
    # 拿不到 CancelledError）。取消已经跑完、控制面的 `_running` 也空了，此刻走
    # `cancel_task` 正好命中「不在跑」那条：写 evidence cancelled、卡片置 cancelled、
    # 把 Gateway 手上的沙箱还掉。
    for task, _session in stranded:
        with contextlib.suppress(Exception):
            await app.plane.cancel_task(task, notify=False)

    if app.sandbox is not None:
        aclose = getattr(app.sandbox, "aclose", None)
        if callable(aclose):
            with contextlib.suppress(Exception):
                await aclose()

    close = getattr(app.store, "close", None)
    if callable(close):
        with contextlib.suppress(Exception):
            await close()
    log.info("aite.down")


def _pending(app: AiteApp) -> int:
    value = getattr(app.plane, "pending", 0)
    return value if isinstance(value, int) else 0


# --------------------------------------------------------------------------
# 命令行
# --------------------------------------------------------------------------

def _build_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(
        prog="python -m aite.app",
        description="Aite P0 常驻进程：飞书长连接 + 进程内队列 + SQLite（单副本）",
    )
    ap.add_argument(
        "--config",
        default=str(DEFAULT_CONFIG_PATH),
        help="配置文件路径（默认 config/aite.yaml；样例见 config/aite.example.yaml）",
    )
    ap.add_argument(
        "--traceback",
        action="store_true",
        help="起不来时把异常栈打到 stderr（默认只打一行人话）",
    )
    return ap


def _main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s %(message)s",
        stream=sys.stderr,
    )

    try:
        app = build_app(load_config(args.config))
    except Exception as exc:                      # 起飞前体检：一律人话 + 非零退出码
        print(f"aite 起不来：{_reason(exc)}", file=sys.stderr)
        print(f"用的配置是 {args.config}（样例见 config/aite.example.yaml）", file=sys.stderr)
        if args.traceback:
            traceback.print_exception(exc, file=sys.stderr)
        return EXIT_STARTUP

    try:
        asyncio.run(run_app(app))
    except KeyboardInterrupt:                     # 信号没挂上时的兜底（非主线程等）
        return EXIT_HARD_STOP
    return 0


def _reason(exc: BaseException) -> str:
    """异常 -> 给人看的一句话。`StartupError` 的消息本身就是那句话，别再套类名。"""
    if isinstance(exc, StartupError):
        return str(exc)
    text = str(exc).strip()
    return f"{type(exc).__name__}: {text}" if text else type(exc).__name__


if __name__ == "__main__":
    main()
