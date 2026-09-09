"""SandboxPort 的 Docker 实现（owner: T3，契约见 aite/contracts/ports.py §3.2）。

按 §3.2 / §6-T3 那几行落地的要点：

- `acquire` 建的容器一定带标签 `aite.task=<task_id>`（B4 靠
  `docker ps -a --filter label=aite.task` 验收），`network=none`
  —— 凭证与平台调用都不在沙箱里发生（§3.1 SandboxSpec.network 是 Literal["none"]）。
- `/work` 是唯一工作面：`put_file` / `get_file` 的 path 必须在它下面，越界抛
  `SandboxPathError`（归一化在 `workdir.py` 里，是纯函数）。
- `exec` 的超时**在容器内**由 coreutils `timeout` 强制执行，超时退出码统一成
  `EXEC_TIMEOUT_EXIT_CODE`(124)。这样即使调用方那边的 `asyncio.wait_for` 先取消，
  容器里的进程也已经被杀掉了，不会留一个跑满 CPU 的孤儿。
- stdout / stderr 各自超 `MAX_EXEC_OUTPUT_CHARS` 截断并置 `truncated=True`。
- `files_out` 是本次 exec 新增 / 修改的 `/work` 下文件（跑之前后各拍一次
  size+mtime_ns 快照做差集）。
- `release` 幂等；`touch` 刷新最近活动时间；`reap_idle` 释放空闲超时的容器
  并返回被释放的 id。`reap_idle` 除了自己记账的容器，还会扫标签捡上一次进程
  留下的孤儿 —— 这条是 B4「reap 之后 docker ps -a 必须为空」的兜底。

docker SDK 是同步的，所有阻塞调用都用 `asyncio.to_thread` 挪出事件循环。
"""
import asyncio
import io
import json
import tarfile
import time
import uuid
from dataclasses import dataclass, field
from datetime import UTC, datetime

# 只从子模块 import：仓库根有个 docker/ 目录（§3.4 归 T3 的镜像上下文），
# 顶层 `import docker` 会被 ruff 的 isort 当成一方包，import 块永远排不齐。
# DockerClient.from_env() 就是 docker.from_env() 的本体。
from docker.client import DockerClient
from docker.errors import DockerException, ImageNotFound, NotFound

from ..contracts import (
    MAX_EXEC_OUTPUT_CHARS,
    ExecRequest,
    ExecResult,
    FileEntry,
    SandboxSpec,
)
from .errors import SandboxError, SandboxFileNotFound, SandboxNotFound
from .workdir import EXEC_TIMEOUT_EXIT_CODE, WORKDIR, require_file_path

#: 容器标签键。B4 的验收命令写死了它：`docker ps -a --filter label=aite.task`。
LABEL_TASK = "aite.task"
#: 再打一个「这是 Aite 管的」标记，方便运维一眼分辨（不参与任何判据）。
LABEL_MANAGED = "aite.managed"

#: 容器里探路用的环境变量名，避免把 workdir 拼进 shell 命令。
_ENV_WORKDIR = "AITE_WORKDIR"

_READY_CODE = """
import os, shutil
wd = os.environ.get("AITE_WORKDIR", "/work")
bad = []
if shutil.which("timeout") is None:
    bad.append("镜像里没有 coreutils timeout，沙箱无法强制执行超时")
if not os.path.isdir(wd):
    bad.append("工作目录不存在：" + wd)
elif not os.access(wd, os.W_OK):
    bad.append("工作目录不可写：" + wd)
print("AITE_SANDBOX_OK" if not bad else "AITE_SANDBOX_BAD: " + "; ".join(bad))
"""

_SNAPSHOT_CODE = """
import json, os, sys
root = os.environ.get("AITE_WORKDIR", "/work")
out = {}
for folder, _dirs, files in os.walk(root):
    for name in files:
        path = os.path.join(folder, name)
        try:
            st = os.stat(path)
        except OSError:
            continue
        out[path] = [st.st_size, st.st_mtime_ns]
json.dump(out, sys.stdout)
"""

_READY_MARK = "AITE_SANDBOX_OK"


def _clip(text: str, limit: int = MAX_EXEC_OUTPUT_CHARS) -> tuple[str, bool]:
    """超长就截断，返回 (文本, 是否截断过)。"""
    if len(text) <= limit:
        return text, False
    marker = "\n…[输出已截断]"
    keep = max(0, limit - len(marker))
    return text[:keep] + marker, True


def _parse_docker_time(value: str | None) -> datetime | None:
    """解析 Docker 的 RFC3339 纳秒时间戳（如 2026-09-09T12:00:00.123456789Z）。"""
    if not value:
        return None
    text = value.strip().rstrip("Z")
    if not text or text.startswith("0001-01-01"):        # Docker 用它表示「没发生过」
        return None
    if "." in text:
        head, frac = text.split(".", 1)
        text = f"{head}.{(frac + '000000')[:6]}"
    try:
        return datetime.fromisoformat(text).replace(tzinfo=UTC)
    except ValueError:
        return None


@dataclass
class _Box:
    """一个活着的沙箱在本进程里的记账。"""

    sandbox_id: str
    task_id: str
    workdir: str
    last_active: float = field(default_factory=time.monotonic)

    def touch(self) -> None:
        self.last_active = time.monotonic()

    def idle_for(self) -> float:
        return time.monotonic() - self.last_active


class DockerSandbox:
    """SandboxPort（T3）。

    一个 task 一个容器。谁 acquire 谁负责给 sandbox_id 找地方存（P0 里是
    `P0ToolGateway` 按 task_id 记着），本类只管容器本身的生死与 /work 的读写。
    """

    def __init__(
        self,
        *,
        client: DockerClient | None = None,
        file_uid: int = 1000,
        file_gid: int = 1000,
        keepalive_cmd: list[str] | None = None,
        kill_grace_sec: int = 2,
    ) -> None:
        self._client = client
        self._client_lock = asyncio.Lock()
        # put_file 落到容器里的文件归谁：镜像里跑的是 uid 1000 的 aite 用户，
        # 文件写成 root 的话沙箱里的代码就改不动它。
        self._file_uid = file_uid
        self._file_gid = file_gid
        self._keepalive_cmd = keepalive_cmd or ["sleep", "infinity"]
        self._kill_grace_sec = kill_grace_sec
        self._boxes: dict[str, _Box] = {}

    # ------------------------------------------------------------------ SandboxPort

    async def acquire(self, task_id: str, spec: SandboxSpec) -> str:
        client = await self._get_client()
        try:
            await asyncio.to_thread(client.images.get, spec.image)
        except ImageNotFound as exc:
            raise SandboxError(
                f"沙箱镜像不存在：{spec.image}。先跑 `docker build -t {spec.image} docker/sandbox`"
            ) from exc
        except DockerException as exc:
            raise SandboxError(f"查沙箱镜像失败：{exc}") from exc

        try:
            container = await asyncio.to_thread(
                client.containers.create,
                image=spec.image,
                command=self._keepalive_cmd,
                labels={LABEL_TASK: task_id, LABEL_MANAGED: "p0"},
                working_dir=spec.workdir,
                # §3.1：P0 沙箱无网络。SandboxSpec.network 是 Literal["none"]，
                # 这里照样按值转，别人以后放开这个字段时不会静默连上网。
                network_mode="none" if spec.network == "none" else spec.network,
                mem_limit=f"{spec.mem_mb}m",
                nano_cpus=int(spec.cpu * 1_000_000_000),
                pids_limit=256,
                cap_drop=["ALL"],
                security_opt=["no-new-privileges:true"],
                environment={_ENV_WORKDIR: spec.workdir},
                tty=False,
                stdin_open=False,
                detach=True,
            )
            await asyncio.to_thread(container.start)
        except DockerException as exc:
            raise SandboxError(f"创建沙箱容器失败（task={task_id}）：{exc}") from exc

        sandbox_id = container.id
        box = _Box(sandbox_id=sandbox_id, task_id=task_id, workdir=spec.workdir)
        self._boxes[sandbox_id] = box
        try:
            await asyncio.to_thread(self._check_ready, container, spec.workdir)
        except Exception:
            # 探路不过就别把半残的容器留给调用方 —— 它连超时都保证不了。
            await self.release(sandbox_id)
            raise
        return sandbox_id

    async def exec(self, sandbox_id: str, req: ExecRequest) -> ExecResult:
        box = self._require_box(sandbox_id)
        container = await self._container(sandbox_id)
        started = time.perf_counter()

        before = await asyncio.to_thread(self._snapshot, container, box.workdir)

        # 用户代码落到 /tmp（不在 /work 下），否则它自己会出现在 files_out 里。
        script = f"/tmp/aite_exec_{uuid.uuid4().hex}.py"
        await asyncio.to_thread(
            self._put_bytes, container, script, req.code.encode("utf-8"), 0o600
        )

        timeout_sec = max(1, int(req.timeout_sec))
        cmd = [
            "timeout",
            "-k",
            str(self._kill_grace_sec),
            str(timeout_sec),
            "python",
            "-u",
            script,
        ]
        cmd_started = time.perf_counter()
        exit_code, stdout, stderr = await asyncio.to_thread(
            self._exec_run, container, cmd, box.workdir
        )
        cmd_elapsed = time.perf_counter() - cmd_started

        after = await asyncio.to_thread(self._snapshot, container, box.workdir)
        await asyncio.to_thread(self._rm_quiet, container, script)
        box.touch()

        # `timeout` 正常超时给 124；进程扛住 SIGTERM 被 -k 补的 SIGKILL 干掉时给 137，
        # 而 137 也可能是 OOM，所以再用「跑够了时长」这一条把两者分开。
        timed_out = exit_code == EXEC_TIMEOUT_EXIT_CODE or (
            exit_code == 137 and cmd_elapsed >= timeout_sec
        )
        if timed_out:
            exit_code = EXEC_TIMEOUT_EXIT_CODE
            note = f"[aite] 执行超过 {timeout_sec}s 上限，已在沙箱内终止"
            stderr = f"{stderr.rstrip()}\n{note}" if stderr.strip() else note

        stdout, cut_out = _clip(stdout)
        stderr, cut_err = _clip(stderr)

        return ExecResult(
            exit_code=exit_code,
            stdout=stdout,
            stderr=stderr,
            duration_ms=int((time.perf_counter() - started) * 1000),
            truncated=cut_out or cut_err,
            files_out=_diff_files(before, after),
        )

    async def put_file(self, sandbox_id: str, path: str, data: bytes) -> None:
        box = self._require_box(sandbox_id)
        target = require_file_path(path, workdir=box.workdir)
        container = await self._container(sandbox_id)
        await asyncio.to_thread(self._put_bytes, container, str(target), data, 0o644)
        box.touch()

    async def get_file(self, sandbox_id: str, path: str) -> bytes:
        box = self._require_box(sandbox_id)
        target = require_file_path(path, workdir=box.workdir)
        container = await self._container(sandbox_id)
        data = await asyncio.to_thread(self._get_bytes, container, str(target))
        box.touch()
        return data

    async def list_files(self, sandbox_id: str) -> list[str]:
        box = self._require_box(sandbox_id)
        container = await self._container(sandbox_id)
        snapshot = await asyncio.to_thread(self._snapshot, container, box.workdir)
        return sorted(snapshot)

    async def touch(self, sandbox_id: str) -> None:
        # 刷新活动时间纯属提示性动作：沙箱早就没了也不该把调用方炸掉
        # （§3.6 W7 的 reaper 与 worker 是两条腿，谁先谁后不定）。
        box = self._boxes.get(sandbox_id)
        if box is not None:
            box.touch()

    async def release(self, sandbox_id: str) -> None:
        """幂等：不认识的 id、已经没了的容器，都当成已经释放。"""
        self._boxes.pop(sandbox_id, None)
        try:
            client = await self._get_client()
            container = await asyncio.to_thread(client.containers.get, sandbox_id)
            await asyncio.to_thread(container.remove, force=True)
        except NotFound:
            return
        except DockerException as exc:
            raise SandboxError(f"释放沙箱失败（{sandbox_id[:12]}）：{exc}") from exc

    async def reap_idle(self, idle_sec: int) -> list[str]:
        """释放空闲超过 idle_sec 的沙箱，返回被释放的 sandbox_id。"""
        victims = [sid for sid, box in self._boxes.items() if box.idle_for() >= idle_sec]
        victims += await self._orphans(idle_sec, known=set(self._boxes))

        released: list[str] = []
        for sid in dict.fromkeys(victims):
            await self.release(sid)
            released.append(sid)
        return released

    # ------------------------------------------------------------------ 附加（非契约）

    async def aclose(self) -> None:
        """释放本进程记着的全部沙箱并关掉 docker 客户端。"""
        for sid in list(self._boxes):
            await self.release(sid)
        client, self._client = self._client, None
        if client is not None:
            await asyncio.to_thread(client.close)

    # ------------------------------------------------------------------ 内部

    async def _get_client(self) -> DockerClient:
        if self._client is not None:
            return self._client
        async with self._client_lock:
            if self._client is None:
                try:
                    self._client = await asyncio.to_thread(DockerClient.from_env)
                except DockerException as exc:
                    raise SandboxError(f"连不上 Docker daemon：{exc}") from exc
        return self._client

    def _require_box(self, sandbox_id: str) -> _Box:
        box = self._boxes.get(sandbox_id)
        if box is None:
            raise SandboxNotFound(f"不认识的 sandbox_id：{sandbox_id!r}（没 acquire 过，或已 release）")
        return box

    async def _container(self, sandbox_id: str):
        client = await self._get_client()
        try:
            return await asyncio.to_thread(client.containers.get, sandbox_id)
        except NotFound as exc:
            self._boxes.pop(sandbox_id, None)
            raise SandboxNotFound(f"沙箱容器已经不在了：{sandbox_id[:12]}") from exc
        except DockerException as exc:
            raise SandboxError(f"取沙箱容器失败（{sandbox_id[:12]}）：{exc}") from exc

    async def _orphans(self, idle_sec: int, *, known: set[str]) -> list[str]:
        """扫标签找上一次进程留下的孤儿容器 —— 本进程没记账，只能按容器时间算空闲。"""
        client = await self._get_client()
        try:
            containers = await asyncio.to_thread(
                client.containers.list, all=True, filters={"label": LABEL_TASK}
            )
        except DockerException:
            return []

        now = datetime.now(UTC)
        out: list[str] = []
        for container in containers:
            if container.id in known:
                continue
            state = container.attrs.get("State", {}) or {}
            stamps = [
                _parse_docker_time(container.attrs.get("Created")),
                _parse_docker_time(state.get("StartedAt")),
                _parse_docker_time(state.get("FinishedAt")),
            ]
            seen = [s for s in stamps if s is not None]
            if not seen or (now - max(seen)).total_seconds() >= idle_sec:
                out.append(container.id)
        return out

    # --- 下面这些是同步的 docker SDK 调用，一律由 to_thread 包着进来 ---

    def _exec_run(self, container, cmd: list[str], workdir: str) -> tuple[int, str, str]:
        try:
            result = container.exec_run(cmd=cmd, workdir=workdir, demux=True)
        except DockerException as exc:
            raise SandboxError(f"在沙箱里执行失败：{exc}") from exc
        raw_out, raw_err = result.output if result.output else (None, None)
        stdout = (raw_out or b"").decode("utf-8", "replace")
        stderr = (raw_err or b"").decode("utf-8", "replace")
        exit_code = -1 if result.exit_code is None else int(result.exit_code)
        return exit_code, stdout, stderr

    def _check_ready(self, container, workdir: str) -> None:
        exit_code, stdout, stderr = self._exec_run(
            container, ["python", "-c", _READY_CODE], workdir
        )
        if exit_code != 0 or _READY_MARK not in stdout:
            detail = (stdout + stderr).strip() or f"exit_code={exit_code}"
            raise SandboxError(f"沙箱镜像不满足 P0 要求：{detail}")

    def _snapshot(self, container, workdir: str) -> dict[str, list[int]]:
        exit_code, stdout, stderr = self._exec_run(
            container, ["python", "-c", _SNAPSHOT_CODE], workdir
        )
        if exit_code != 0:
            raise SandboxError(f"给 {workdir} 拍文件快照失败：{(stderr or stdout).strip()}")
        try:
            return json.loads(stdout or "{}")
        except json.JSONDecodeError as exc:
            raise SandboxError(f"文件快照不是合法 JSON：{stdout[:200]!r}") from exc

    def _put_bytes(self, container, path: str, data: bytes, mode: int) -> None:
        posix = path.rsplit("/", 1)
        parent = posix[0] or "/"
        name = posix[1]
        exit_code, stdout, stderr = self._exec_run(container, ["mkdir", "-p", parent], "/")
        if exit_code != 0:
            raise SandboxError(f"在沙箱里建目录 {parent} 失败：{(stderr or stdout).strip()}")

        buf = io.BytesIO()
        with tarfile.open(fileobj=buf, mode="w") as tar:
            info = tarfile.TarInfo(name=name)
            info.size = len(data)
            info.mode = mode
            info.uid = self._file_uid
            info.gid = self._file_gid
            info.mtime = int(time.time())
            tar.addfile(info, io.BytesIO(data))
        try:
            ok = container.put_archive(parent, buf.getvalue())
        except DockerException as exc:
            raise SandboxError(f"写入沙箱 {path} 失败：{exc}") from exc
        if not ok:
            raise SandboxError(f"写入沙箱 {path} 失败：Docker 拒绝了这次 put_archive")

    def _get_bytes(self, container, path: str) -> bytes:
        try:
            bits, _stat = container.get_archive(path)
        except NotFound as exc:
            raise SandboxFileNotFound(f"沙箱里没有这个文件：{path}") from exc
        except DockerException as exc:
            raise SandboxError(f"从沙箱读 {path} 失败：{exc}") from exc

        buf = io.BytesIO(b"".join(bits))
        with tarfile.open(fileobj=buf) as tar:
            member = next((m for m in tar.getmembers() if m.isfile()), None)
            if member is None:
                raise SandboxFileNotFound(f"{path} 不是一个文件")
            handle = tar.extractfile(member)
            if handle is None:
                raise SandboxFileNotFound(f"{path} 读不出内容")
            return handle.read()

    def _rm_quiet(self, container, path: str) -> None:
        try:
            self._exec_run(container, ["rm", "-f", path], "/")
        except SandboxError:
            pass        # 清理临时脚本失败无所谓，容器整个会被 reap 掉


def _diff_files(
    before: dict[str, list[int]], after: dict[str, list[int]]
) -> list[FileEntry]:
    """本次 exec 新增 / 修改的文件（比 size + mtime_ns）。"""
    return [
        FileEntry(path=path, size=int(meta[0]))
        for path, meta in sorted(after.items())
        if before.get(path) != meta
    ]


__all__ = ["LABEL_MANAGED", "LABEL_TASK", "WORKDIR", "DockerSandbox"]
