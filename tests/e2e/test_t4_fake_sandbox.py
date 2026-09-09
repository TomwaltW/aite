"""FakeSandbox 自测：内存 FS、exec 脚本、release 幂等、reap_idle 可确定性触发。"""
from __future__ import annotations

import inspect

import pytest

from aite.contracts import ExecRequest, SandboxSpec
from aite.contracts.ports import SandboxPort
from aite.testing import FakeSandbox, FakeSandboxError
from aite.testing.samples import PNG_MAGIC

SPEC = SandboxSpec(image="aite-sandbox:p0")


def test_implements_every_sandbox_port_method():
    want = {n for n, o in vars(SandboxPort).items() if not n.startswith("_") and inspect.isfunction(o)}
    for name in want:
        fn = getattr(FakeSandbox, name, None)
        assert fn is not None and inspect.iscoroutinefunction(fn), f"缺 {name} 或不是 async"


async def test_acquire_put_get_list():
    sb = FakeSandbox()
    sid = await sb.acquire("task-1", SPEC)
    await sb.put_file(sid, "/work/in/a.csv", b"month,amount")
    assert await sb.get_file(sid, "/work/in/a.csv") == b"month,amount"
    assert await sb.list_files(sid) == ["/work/in/a.csv"]


async def test_put_file_outside_work_is_rejected():
    """§3.2 put_file 注释：path 必须在 /work 下。"""
    sb = FakeSandbox()
    sid = await sb.acquire("task-1", SPEC)
    with pytest.raises(FakeSandboxError, match="/work"):
        await sb.put_file(sid, "/etc/passwd", b"x")


async def test_get_missing_file_raises_with_listing():
    sb = FakeSandbox()
    sid = await sb.acquire("task-1", SPEC)
    with pytest.raises(FileNotFoundError, match="没有 /work/out.png"):
        await sb.get_file(sid, "/work/out.png")


async def test_exec_script_matches_by_substring_and_writes_files():
    """04_csv_to_chart 的关键：代码里有 savefig 就产出一张真 PNG。"""
    sb = FakeSandbox(exec_script=[{"match": "savefig", "stdout": "ok",
                                   "writes": {"/work/out.png": "builtin:png"}}])
    sid = await sb.acquire("task-1", SPEC)
    res = await sb.exec(sid, ExecRequest(code="plt.savefig('/work/out.png')"))
    assert res.exit_code == 0 and res.stdout == "ok"
    assert [f.path for f in res.files_out] == ["/work/out.png"]
    assert (await sb.get_file(sid, "/work/out.png"))[:8] == PNG_MAGIC


async def test_unmatched_code_gets_a_harmless_default():
    sb = FakeSandbox(exec_script=[{"match": "savefig", "stdout": "ok"}])
    sid = await sb.acquire("task-1", SPEC)
    res = await sb.exec(sid, ExecRequest(code="print(1)"))
    assert (res.exit_code, res.stdout, res.files_out) == (0, "", [])


async def test_exec_script_times_limits_reuse():
    sb = FakeSandbox(exec_script=[{"stdout": "第一次", "times": 1}, {"stdout": "之后"}])
    sid = await sb.acquire("task-1", SPEC)
    assert (await sb.exec(sid, ExecRequest(code="x"))).stdout == "第一次"
    assert (await sb.exec(sid, ExecRequest(code="x"))).stdout == "之后"


async def test_exec_error_step_raises_sandbox_error():
    """§3.3「沙箱创建/执行失败」→ Gateway 要把它翻成 code=sandbox。"""
    sb = FakeSandbox(exec_script=[{"error": "Docker daemon 不可用"}])
    sid = await sb.acquire("task-1", SPEC)
    with pytest.raises(FakeSandboxError, match="Docker daemon"):
        await sb.exec(sid, ExecRequest(code="x"))


async def test_release_is_idempotent():
    sb = FakeSandbox()
    sid = await sb.acquire("task-1", SPEC)
    await sb.release(sid)
    await sb.release(sid)
    assert sb.released_ids == [sid]
    assert sb.alive == []


async def test_released_sandbox_cannot_be_used():
    sb = FakeSandbox()
    sid = await sb.acquire("task-1", SPEC)
    await sb.release(sid)
    with pytest.raises(FakeSandboxError, match="已经 release"):
        await sb.exec(sid, ExecRequest(code="x"))


async def test_reap_idle_uses_injectable_clock():
    """不用真等 5 分钟：时钟可注入，reap 的判定就可确定性触发。"""
    now = [1000.0]
    sb = FakeSandbox(clock=lambda: now[0])
    idle = await sb.acquire("task-idle", SPEC)
    busy = await sb.acquire("task-busy", SPEC)

    now[0] += 400
    await sb.touch(busy)
    assert await sb.reap_idle(300) == [idle]
    assert sb.alive == [busy]
    assert await sb.reap_idle(300) == []          # 已释放的不会被重复收割


async def test_unknown_sandbox_id_is_reported_with_registry():
    sb = FakeSandbox()
    with pytest.raises(FakeSandboxError, match="未知 sandbox_id"):
        await sb.list_files("sb-nope")
