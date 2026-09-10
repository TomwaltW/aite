"""`--sandbox docker` 那一档的零件（owner: T23，dev-spec §2.4 M3 / §3.8 04_csv_to_chart）。

评测默认跑的是全替身：`FakeSandbox` 按 `exec_script` 演、`FakeToolGateway` 自己实现工具。
这一档换成**真** `DockerSandbox` + **真** `P0ToolGateway`，好让「附件 → download_attachment
→ run_python 画图 → final(artifacts) → send_file」这条 M3 主干在评测里真的走一遍。
平台仍然是 `FakePlatform` —— 附件从它来，回执也发回它，验的是沙箱与 Gateway 这一段。

两件事得在这里补上，都不是别人漏了，是「只有接线方才知道」的：

**一、断言面。** 场景的 `expect` 读 `deps.sandbox.calls` / `deps.gateway.count()` /
`deps.gateway.results_of()` —— 那些是替身的记账面，真实现没有（协议 `SandboxPort` /
`ToolGateway` 里也确实没有，它们不是契约的一部分）。所以这里给两个透明包装
`SandboxProbe` / `GatewayProbe`，跟 `ModelProbe` 之于 live 模型是同一件事：照原样转发，
顺手记账，未知属性透传。不套的话 docker 档第一条断言就 AttributeError。

**二、`session_token`。** `P0ToolGateway` 的令牌校验是失败关闭的（见它的模块 docstring）：
`register_task()` 现场登记，或构造时给 `token_resolver`，两个都没有 = 每个工具调用都
`denied`。真机那条路走前者（`aite/app.py` 的 `AppWorker.run` 在任务开跑那一刻登记），
而评测这边接不上：`wiring.build_control_plane` 是**运行时发现** ControlPlane 的，
`PARAM_ALIASES` 里没有 `worker` 这一格，`InProcessControlPlane` 于是自己造一个普通
`AgentWorker` —— 没有 `AppWorker` 那三行。所以这边走后者：`token_resolver` 直接读
`FakeSessionStore.tasks`（同步 dict，`Task.session_token` 就在上面），见 `token_resolver_of`。
"""
from __future__ import annotations

from collections.abc import Callable, Iterable
from typing import Any

from ..contracts import (
    AiteConfig,
    ExecRequest,
    ExecResult,
    SandboxSpec,
    ToolCallRequest,
    ToolContext,
    ToolResult,
    ToolSpec,
)
from ..testing.fake_store import FakeSessionStore
from ..testing.recorder import CallLog

__all__ = [
    "GatewayProbe",
    "SandboxProbe",
    "build_docker_stack",
    "docker_preflight",
    "sandbox_spec_of",
    "scenarios_with_exec_script",
    "token_resolver_of",
]


# --------------------------------------------------------------------------
# 透明记账包装
# --------------------------------------------------------------------------

class SandboxProbe:
    """`SandboxPort` 的透明包装：照原样转发，顺手把每次调用记进 `calls`。

    方法名与参数名跟 `FakeSandbox` 记的那套逐字对齐 —— `check: sandbox_calls` 的
    `method: release` 这类断言两档下读到的是同一个键。未知属性透传给被包的实现。

    **`in_flight` 是这一档能跑起来的前提**，跟 `ModelProbe.in_flight` 之于 live 模型
    一模一样：`wiring.settle()` 的静默判据是「所有替身的记账都不再变」，它看不见长时间
    的 await。替身沙箱瞬时返回，这从来没露过馅；真沙箱 `acquire` 一次要起容器 + 探路
    一秒多，这期间一个替身都不会被碰 —— 照 150ms 的判据，任务在第一个容器建好之前
    就被判定「不干活了」，`stop_loop` 当场把它取消。实测过：`04` 的 `duration_ms` 停在
    169ms，容器建好随即被收走，七条断言全红。

    调用记在**执行前**（`ModelProbe` 同理）：抛出去的那次也得留下痕迹，不然错在哪
    数都数不出来。
    """

    def __init__(self, inner: Any) -> None:
        self.inner = inner
        self.calls = CallLog()
        self._in_flight = 0

    @property
    def in_flight(self) -> int:
        """还没返回的调用数。`wiring.sandbox_busy` 读它。"""
        return self._in_flight

    async def _run(self, call: Any, coro: Any) -> Any:
        self._in_flight += 1
        try:
            result = await coro
        except Exception as exc:
            call.error = f"{type(exc).__name__}: {exc}"
            raise
        finally:
            self._in_flight -= 1
        return result

    # ---- SandboxPort ----------------------------------------------------

    async def acquire(self, task_id: str, spec: SandboxSpec) -> str:
        call = self.calls.record("acquire", task_id=task_id, image=spec.image)
        sandbox_id = await self._run(call, self.inner.acquire(task_id, spec))
        call.result = sandbox_id
        return sandbox_id

    async def exec(self, sandbox_id: str, req: ExecRequest) -> ExecResult:
        call = self.calls.record("exec", sandbox_id=sandbox_id, timeout_sec=req.timeout_sec)
        result = await self._run(call, self.inner.exec(sandbox_id, req))
        call.result = f"exit_code={result.exit_code}"
        return result

    async def put_file(self, sandbox_id: str, path: str, data: bytes) -> None:
        call = self.calls.record("put_file", sandbox_id=sandbox_id, path=path, size=len(data))
        await self._run(call, self.inner.put_file(sandbox_id, path, data))

    async def get_file(self, sandbox_id: str, path: str) -> bytes:
        call = self.calls.record("get_file", sandbox_id=sandbox_id, path=path)
        data = await self._run(call, self.inner.get_file(sandbox_id, path))
        call.result = f"{len(data)} bytes"
        return data

    async def list_files(self, sandbox_id: str) -> list[str]:
        call = self.calls.record("list_files", sandbox_id=sandbox_id)
        return await self._run(call, self.inner.list_files(sandbox_id))

    async def touch(self, sandbox_id: str) -> None:
        call = self.calls.record("touch", sandbox_id=sandbox_id)
        await self._run(call, self.inner.touch(sandbox_id))

    async def release(self, sandbox_id: str) -> None:
        call = self.calls.record("release", sandbox_id=sandbox_id)
        await self._run(call, self.inner.release(sandbox_id))

    async def reap_idle(self, idle_sec: int) -> list[str]:
        call = self.calls.record("reap_idle", idle_sec=idle_sec)
        return await self._run(call, self.inner.reap_idle(idle_sec))

    # ---- 附加（非契约）--------------------------------------------------

    async def aclose(self) -> None:
        """把本进程还开着的容器收干净。评测 runner 在场景收尾时调，见 `Deps.aclose`。

        `DockerSandbox.aclose` 顺带关掉 docker 客户端，所以一个探针只服务一个场景。
        """
        self.calls.record("aclose")
        await self.inner.aclose()

    def __getattr__(self, name: str) -> Any:
        return getattr(self.inner, name)


class GatewayProbe:
    """`ToolGateway` 的透明包装：转发 `catalog` / `call`，补上断言要的记账面。

    `count()` / `results_of()` / `calls` 与 `FakeToolGateway` 同名同义 —— `checks.py`
    的 `gateway_calls` / `gateway_result` 两个 check 两档下读到的是同一套东西。
    `release_task` / `sandbox_id_of` / `register_task` 这些 worker 与控制面会 `getattr`
    的方法走 `__getattr__` 透传给真实现。
    """

    def __init__(self, inner: Any) -> None:
        self.inner = inner
        self.calls = CallLog()
        self.results: list[ToolResult] = []

    # ---- ToolGateway ----------------------------------------------------

    def catalog(self, ctx: ToolContext) -> list[ToolSpec]:
        self.calls.record("catalog", task_id=ctx.task_id)
        return self.inner.catalog(ctx)

    async def call(self, ctx: ToolContext, req: ToolCallRequest) -> ToolResult:
        call = self.calls.record(
            "call", name=req.name, task_id=ctx.task_id, arguments=req.arguments
        )
        result = await self.inner.call(ctx, req)
        call.result = f"ok={result.ok}" + ("" if result.error is None else f" {result.error.code}")
        self.results.append(result)
        return result

    # ---- 给断言用 ------------------------------------------------------

    def count(self, name: str) -> int:
        return sum(1 for c in self.calls.of("call") if c.kwargs.get("name") == name)

    def results_of(self, name: str) -> list[ToolResult]:
        return [r for r in self.results if r.name == name]

    def __getattr__(self, name: str) -> Any:
        return getattr(self.inner, name)


# --------------------------------------------------------------------------
# 造这一档
# --------------------------------------------------------------------------

def sandbox_spec_of(config: AiteConfig) -> SandboxSpec:
    """跟 `aite/app.py:_sandbox_spec` 同一口径 —— 真机怎么从 config 出 spec，这里就怎么出。"""
    cfg = config.sandbox
    return SandboxSpec(image=cfg.image, cpu=cfg.cpu, mem_mb=cfg.mem_mb)


def token_resolver_of(store: FakeSessionStore) -> Callable[[str], str | None]:
    """task_id -> `Task.session_token`，直接读 store 的 dict。

    两点讲究：

    * **同步**。`TokenResolver` 是同步签名，而 `SessionStore.get_task` 是 async ——
      真机那条路因此接不上 resolver，只能在 `AppWorker.run` 里现场 `register_task`。
      替身 store 把任务摊在 `tasks` 这个 dict 上，这边正好接得上。
    * **不走 store 的方法**。`get_task()` 会往 `store.calls` 记一笔，而 `Deps.activity()`
      数的就是这些记账 —— 每次工具调用都去碰它，`settle()` 的静默判据就永远不静默。
      跟 `wiring.newest_task` 直接读字段是同一个理由。
    """

    def resolve(task_id: str) -> str | None:
        task = store.tasks.get(task_id)
        return task.session_token if task is not None else None

    return resolve


def build_docker_stack(
    *,
    platform: Any,
    store: FakeSessionStore,
    config: AiteConfig,
) -> tuple[SandboxProbe, GatewayProbe]:
    """造真 `DockerSandbox` + 真 `P0ToolGateway`，各套一层探针。

    这里**不碰 Docker daemon**：`DockerSandbox` 的客户端是首次 `acquire` 才建的，
    跟 `aite/app.py:_build_sandbox` 一样只做组装。daemon 在不在由 `docker_preflight`
    在起飞前查，那样报出来的是一行人话而不是十个场景各自烂在第一个工具调用上。
    """
    # 延迟 import：默认档（fake）的调用方不必把 docker SDK 一起拖起来。
    from ..gateway.tool_gateway import P0ToolGateway
    from ..sandbox.docker_sandbox import DockerSandbox

    sandbox = SandboxProbe(DockerSandbox())
    gateway = GatewayProbe(
        P0ToolGateway(
            platform=platform,
            sandbox=sandbox,
            sandbox_spec=sandbox_spec_of(config),
            token_resolver=token_resolver_of(store),
        )
    )
    return sandbox, gateway


# --------------------------------------------------------------------------
# 起飞前体检
# --------------------------------------------------------------------------

def docker_preflight(images: Iterable[str]) -> str | None:
    """daemon 连得上吗、镜像在吗。没问题回 `None`，有问题回一行给人看的话。

    照 `__main__._live_model_factory` 那条先例办：能在起飞前看出来的问题就在起飞前
    说清楚，别让它变成「十个场景各自跑到第一个 `run_python` 才报 sandbox 错」——
    那时真正的原因（Docker Desktop 没开 / 镜像没 build）一个字都看不到。
    """
    try:
        from docker.client import DockerClient
        from docker.errors import DockerException, ImageNotFound
    except ImportError as exc:                        # 依赖装全了不会走到这
        return f"docker SDK 没装（{exc}）。先 pip install -e '.[dev]'"

    try:
        client = DockerClient.from_env()
    except DockerException as exc:
        return f"连不上 Docker daemon：{exc}。先把 Docker Desktop 起起来，再跑 --sandbox docker"

    try:
        try:
            client.ping()
        except DockerException as exc:
            return f"Docker daemon 没应答：{exc}。先把 Docker Desktop 起起来，再跑 --sandbox docker"
        for image in sorted(set(images)):
            try:
                client.images.get(image)
            except ImageNotFound:
                return (
                    f"沙箱镜像不存在：{image}。先跑 `docker build -t {image} docker/sandbox`"
                )
            except DockerException as exc:
                return f"查沙箱镜像 {image} 失败：{exc}"
    finally:
        try:
            client.close()
        except Exception:                             # 收尾失败不该盖掉体检结论
            pass
    return None


def scenarios_with_exec_script(scenarios: Iterable[Any]) -> list[str]:
    """哪些场景写了 `sandbox.exec_script`。

    那是 `FakeSandbox` 的台词，docker 档下**一律忽略**：代码交给真容器跑，本来就没有
    「照脚本回一个 exit_code」这回事。选忽略而不是报错，是因为唯一带 exec_script 的
    靶心场景就是 04_csv_to_chart —— 报错的话这一档连它都跑不了，这轨的目的就没了。
    忽略但要留痕：`__main__` 拿这个名单在起飞时打一行 stderr 点名。
    """
    return [sc.name for sc in scenarios if sc.sandbox.exec_script]
