"""骨架完整性测试（dev-spec-2026-09-09 §3.4 文件归属表 + §6 的 T0 验收那一行）。

这一份把「目录骨架已经立起来、契约锁与样例配置没跑偏、§3.2 的调用面没漂」
变成常驻机器判据。

刻意**不**断言各 track 的实现类还是空实现：那些文件（aite/adapters/**、aite/control/**、
aite/sandbox/** …）归 T1–TΩ，实现完成后方法会变多、构造要参数、方法体不再 raise。
把「还没实现」冻在 T0 独占、别人只读的 tests/contracts 里，会让 §2.3 的 C1
在每条轨第一次动手时必红，且它们按 §3.4 无权修。T0 阶段的空实现由 T0 回执
一次性核验（§6 T0 行），不做常驻判据。
"""
import ast
import inspect
import subprocess
import sys
import tomllib
from pathlib import Path

import pytest

import aite.contracts as contracts
from aite.config import load_config
from aite.contracts import AiteConfig
from aite.contracts import ports as contract_ports
from aite.contracts.lock import REPO_ROOT

# ---------------------------------------------------------------------------
# §3.4 归属表：逐条抄下来的路径清单（显式列表，不用通配）
# ---------------------------------------------------------------------------

# 表里以 `/**` 结尾的行 —— 断言目录存在
SPEC_DIRS = [
    # owner: T0
    "aite/contracts",
    "tests/contracts",
    # owner: T1
    "aite/adapters/feishu",
    "tests/adapters",
    "tests/fixtures/feishu",
    # owner: T2
    "aite/ingress",
    "aite/control",
    "aite/worker",
    "aite/evidence",
    "tests/control",
    "tests/worker",
    "tests/evidence",
    # owner: T3
    "aite/sandbox",
    "aite/gateway",
    "aite/tools",
    "docker/sandbox",
    "tests/sandbox",
    "tests/gateway",
    # owner: T4
    "aite/models",
    "aite/testing",
    "aite/evals",
    "evals",
    "tests/e2e",
    ".github/workflows",
    "scripts",
    # owner: TΩ
    "tests/integration",
]

# 表里写死到具体文件的行 —— 断言文件存在
SPEC_FILES = [
    "docs/dev-spec-2026-09-09.md",   # 总管
    ".contracts.lock",               # T0
    "pyproject.toml",                # T0
    ".gitignore",                    # T0
    "config/aite.example.yaml",      # T0
    "aite/__init__.py",              # T0
    "aite/config.py",                # T0
    "aite/app.py",                   # T0 stub / TΩ 实现
    "tests/conftest.py",             # T0
    "docker-compose.yml",            # T4
    "Makefile",                      # T4
    "README.md",                     # T4
]


def test_repo_root_is_the_real_repo_root():
    """lock.py 推出来的 REPO_ROOT 必须就是本文件上两级，两条路径口径一致。"""
    assert REPO_ROOT == Path(__file__).resolve().parents[2]
    assert (REPO_ROOT / "pyproject.toml").is_file()


@pytest.mark.parametrize("rel", SPEC_DIRS)
def test_spec_dir_exists(rel):
    """§3.4 里每一条 `路径/**` 对应的目录都必须存在。"""
    p = REPO_ROOT / rel
    assert p.is_dir(), f"§3.4 归属表要求的目录不存在：{rel}"


@pytest.mark.parametrize("rel", SPEC_FILES)
def test_spec_file_exists(rel):
    """§3.4 里每一条具体文件都必须存在。"""
    p = REPO_ROOT / rel
    assert p.is_file(), f"§3.4 归属表要求的文件不存在：{rel}"


def test_spec_path_lists_have_no_duplicates():
    """清单本身不许有重复项，免得「条数够了」掩盖漏抄。"""
    assert len(SPEC_DIRS) == len(set(SPEC_DIRS))
    assert len(SPEC_FILES) == len(set(SPEC_FILES))
    assert set(SPEC_DIRS).isdisjoint(SPEC_FILES)


def test_python_packages_have_init():
    """归属表里的 aite/* 目录都是 Python 包，必须有 __init__.py。"""
    for rel in SPEC_DIRS:
        if not rel.startswith("aite/"):
            continue
        assert (REPO_ROOT / rel / "__init__.py").is_file(), f"{rel} 缺 __init__.py"


# ---------------------------------------------------------------------------
# 契约包本身：版本号与 __all__
# ---------------------------------------------------------------------------

def test_contract_version_is_p0_1():
    """契约版本号写死 p0.1；改版本 = 改契约，必须显式过这条。"""
    assert contracts.CONTRACT_VERSION == "p0.1"


def test_contracts_all_has_no_duplicates():
    """__all__ 不许有重复名字。"""
    dupes = sorted({n for n in contracts.__all__ if contracts.__all__.count(n) > 1})
    assert dupes == [], f"aite/contracts/__init__.py 的 __all__ 有重复项：{dupes}"


def test_contracts_all_names_are_importable():
    """__all__ 里每个名字都能从包里取到，不许有写了名字却没 re-export 的。"""
    missing = [n for n in contracts.__all__ if not hasattr(contracts, n)]
    assert missing == [], f"__all__ 列了但取不到的名字：{missing}"


def test_contract_version_is_exported():
    """CONTRACT_VERSION 自身也要在 __all__ 里，别的 track `from aite.contracts import *` 才拿得到。"""
    assert "CONTRACT_VERSION" in contracts.__all__


# ---------------------------------------------------------------------------
# 契约锁：python -m aite.contracts.lock --check 必须绿
# ---------------------------------------------------------------------------

def test_contracts_lock_check_passes():
    """锁与实际文件一致：退出码 0，stdout 以 OK 开头（形如 `OK 11 files`）。"""
    proc = subprocess.run(
        [sys.executable, "-m", "aite.contracts.lock", "--check"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    assert proc.returncode == 0, f"锁校验失败：\nstdout={proc.stdout}\nstderr={proc.stderr}"
    assert proc.stdout.startswith("OK"), f"stdout 未以 OK 开头：{proc.stdout!r}"
    assert " files" in proc.stdout
    # 条数也要钉死：只验 "OK" 前缀的话，有人从锁里摘掉一个契约文件、
    # 保护面从 11 缩到 10，--check 依旧打印 OK 退出 0，这里必须咬住。
    n_locked = int(proc.stdout.split()[1])
    on_disk = sorted(
        f.relative_to(REPO_ROOT).as_posix()
        for f in (REPO_ROOT / "aite" / "contracts").rglob("*")
        if f.is_file() and "__pycache__" not in f.parts and f.name != "lock.py"
    )
    assert n_locked == len(on_disk) == 11, (
        f"锁里 {n_locked} 个文件，磁盘上 {len(on_disk)} 个（期望 11）：{on_disk}"
    )


# ---------------------------------------------------------------------------

def _diff_config(actual: AiteConfig, expected: AiteConfig) -> list[str]:
    """逐字段（含嵌套一层）比出不等的项，用于失败时把跑偏的字段打出来。"""
    diffs = []
    a, e = actual.model_dump(), expected.model_dump()
    for key in sorted(set(a) | set(e)):
        av, ev = a.get(key), e.get(key)
        if av == ev:
            continue
        if isinstance(av, dict) and isinstance(ev, dict):
            for sub in sorted(set(av) | set(ev)):
                if av.get(sub) != ev.get(sub):
                    diffs.append(f"{key}.{sub}: 样例={av.get(sub)!r} 契约默认={ev.get(sub)!r}")
        else:
            diffs.append(f"{key}: 样例={av!r} 契约默认={ev!r}")
    return diffs


def test_example_config_loads_into_aite_config():
    """load_config 能把样例 YAML 读成 AiteConfig。"""
    cfg = load_config(REPO_ROOT / "config" / "aite.example.yaml")
    assert isinstance(cfg, AiteConfig)


def test_example_config_actually_carries_every_key():
    """先证样例 YAML 真的写了东西 —— 否则下面那条等值断言是自我循环。

    load_config 对空文件返回 AiteConfig() 全默认，于是「样例 == 默认」在样例被
    删空时照样绿。这里直接读原始 YAML，钉住顶层 6 个键与每个嵌套块的字段齐全。"""
    import yaml
    raw = yaml.safe_load((REPO_ROOT / "config" / "aite.example.yaml").read_text(encoding="utf-8"))
    assert isinstance(raw, dict) and raw, "样例配置是空的 —— 等值断言会退化成恒等式"
    assert set(raw) == {"tenant_id", "platform", "feishu", "model", "sandbox", "worker", "storage"}
    expected = AiteConfig().model_dump()
    for block in ("feishu", "model", "sandbox", "worker", "storage"):
        assert set(raw[block]) == set(expected[block]), (
            f"{block} 块的字段与契约不符：样例={sorted(raw[block])} 契约={sorted(expected[block])}"
        )
    # 逐字段的取值也来自 YAML 原文，不经过 pydantic 的默认值兜底
    assert raw["storage"]["sqlite_path"] == "data/aite.db"
    assert raw["worker"]["max_steps"] == 40
    assert raw["model"]["api_key_env"] == "AITE_MODEL_API_KEY"


def test_example_config_equals_contract_defaults():
    """样例配置里的每个值都必须是契约默认值 —— 不等就把跑偏的字段打出来。"""
    cfg = load_config(REPO_ROOT / "config" / "aite.example.yaml")
    expected = AiteConfig()
    diffs = _diff_config(cfg, expected)
    assert diffs == [], "config/aite.example.yaml 与契约默认值不一致：\n  " + "\n  ".join(diffs)
    assert cfg == expected


def test_load_config_rejects_missing_file():
    """配置文件不存在时抛 FileNotFoundError，而不是静默用默认值。"""
    with pytest.raises(FileNotFoundError):
        load_config(REPO_ROOT / "config" / "does-not-exist.yaml")


# ---------------------------------------------------------------------------
# §3.2 契约侧 Protocol 的表面：方法集合与异步/同步形态
#
# 只断言**契约**（aite/contracts/ports.py），不断言任何实现类。
# 各 track 的实现是它们自己白名单里的活，实现完成后方法会变多、构造要参数、
# 方法体不再 raise —— 那是干活，不是回归。把「还没实现」冻进 T0 独占的
# tests/contracts 会让 C1 在每条轨第一次动手时必红，且它们无权修（§3.4 只读）。
# ---------------------------------------------------------------------------

# (契约 Protocol, 该 Protocol 声明的方法名集合)
PORT_PROTOCOLS = [
    (contract_ports.PlatformPort, {
        "start", "stop", "send_text", "send_card", "update_card",
        "send_file", "add_reaction", "read_history", "read_document", "download_file",
    }),
    (contract_ports.SessionStore, {
        "init", "get_session", "find_session_by_thread", "create_session", "update_session",
        "append_turn", "list_turns", "create_task", "update_task", "get_task",
        "list_active_tasks", "next_task_no", "seen_event",
    }),
    (contract_ports.ControlPlane, {"handle_event", "run_forever"}),
    (contract_ports.EvidenceWriter, {"append", "finalize", "verify"}),
    (contract_ports.SandboxPort, {
        "acquire", "exec", "put_file", "get_file", "list_files", "touch", "release", "reap_idle",
    }),
    (contract_ports.ToolGateway, {"catalog", "call"}),
    (contract_ports.ModelPort, {"chat"}),
]

PROTOCOL_IDS = [proto.__name__ for proto, _ in PORT_PROTOCOLS]


def _declared_methods(cls) -> set[str]:
    """类自己声明的公开方法名（不含继承来的、不含私有）。"""
    return {
        name for name, obj in vars(cls).items()
        if not name.startswith("_") and inspect.isfunction(obj)
    }


@pytest.mark.parametrize(("protocol", "expected"), PORT_PROTOCOLS, ids=PROTOCOL_IDS)
def test_port_protocol_method_set_is_frozen(protocol, expected):
    """契约 Protocol 声明的方法集合逐字冻结 —— 谁给 §3.2 加/删方法，这里先红。"""
    assert _declared_methods(protocol) == expected


def test_port_protocol_async_sync_shape_is_frozen():
    """异步/同步形态钉死：§3.2 里只有 EvidenceWriter.verify 与 ToolGateway.catalog 是同步的。"""
    sync_methods = set()
    for protocol, expected in PORT_PROTOCOLS:
        for name in expected:
            if not inspect.iscoroutinefunction(getattr(protocol, name)):
                sync_methods.add(f"{protocol.__name__}.{name}")
    assert sync_methods == {"EvidenceWriter.verify", "ToolGateway.catalog"}


def test_port_protocol_signatures_are_frozen():
    """逐方法冻结签名字符串：参数名、顺序、keyword-only 的 *、默认值、返回标注。

    §3.2 是跨 track 的唯一对话面，签名漂一个字符，两边就对不上了。"""
    frozen = {
        "PlatformPort.read_history":
            "(self, chat_id: str, *, limit: int = 50, thread_id: str | None = None)"
            " -> list[aite.contracts.ports.HistoryMessage]",
        "SessionStore.list_turns":
            "(self, session_id: str, *, limit: int = 200) -> list[aite.contracts.session.Turn]",
        "ModelPort.chat":
            "(self, messages: list[aite.contracts.protocol.Message],"
            " tools: list[aite.contracts.protocol.ToolSpec], *, max_tokens: int,"
            " temperature: float) -> aite.contracts.protocol.ModelTurn",
        "ToolGateway.call":
            "(self, ctx: aite.contracts.gateway.ToolContext,"
            " req: aite.contracts.protocol.ToolCallRequest) -> aite.contracts.gateway.ToolResult",
        "SandboxPort.reap_idle": "(self, idle_sec: int) -> list[str]",
        "EvidenceWriter.append":
            "(self, task_id: str, kind: aite.contracts.evidence.EvidenceKind, payload: dict)"
            " -> aite.contracts.evidence.EvidenceEvent",
        "ControlPlane.handle_event":
            "(self, ev: aite.contracts.events.NormalizedEvent) -> None",
    }
    actual = {}
    for proto, _expected in PORT_PROTOCOLS:
        for name in _declared_methods(proto):
            key = f"{proto.__name__}.{name}"
            if key in frozen:
                actual[key] = str(inspect.signature(getattr(proto, name)))
    assert actual == frozen


# ---------------------------------------------------------------------------
# §3.0 pyproject.toml 冻结块：依赖 / pytest / ruff 三段逐值钉死
#
# 归属表把 pyproject.toml 判给 T0，§3.0 又写明「其他 track 需要新依赖 = 停下来报告」。
# 只断言文件存在挡不住「谁悄悄加了一条 dependency」，所以把 spec 代码块里的值抄成常量比。
# ---------------------------------------------------------------------------

SPEC_30_PROJECT = {
    "name": "aite",
    "version": "0.0.1",
    "requires-python": ">=3.12",
    "dependencies": [
        "pydantic>=2.7",
        "pyyaml>=6",
        "httpx>=0.27",
        "lark-oapi>=1.4",
        "openai>=1.40",
        "docker>=7",
        "aiosqlite>=0.20",
    ],
    "optional-dependencies": {
        "dev": ["pytest>=8", "pytest-asyncio>=0.23", "ruff>=0.5", "respx>=0.21"],
    },
}

SPEC_30_PYTEST_INI = {
    "asyncio_mode": "auto",
    "markers": ["docker: 需要本机 Docker daemon 的测试"],
    "testpaths": ["tests"],
}

SPEC_30_RUFF = {
    "line-length": 110,
    "target-version": "py312",
    "lint": {"select": ["E4", "E7", "E9", "F", "I", "UP"]},
}

# §3.0 之外唯一允许出现的 tool 段：T0 为让 `pip install -e .` 跑通而加的包发现配置。
# 它不改任何契约字段，但也不许再多出别的段 —— 多一段就是有人往冻结文件里塞东西。
ALLOWED_EXTRA_TOOL_KEYS = {"setuptools"}


def _pyproject() -> dict:
    return tomllib.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))


def test_pyproject_project_block_matches_spec_30():
    """[project] 段逐键等于 §3.0 —— 依赖顺序也算，多一条少一条都红。"""
    assert _pyproject()["project"] == SPEC_30_PROJECT


def test_pyproject_dependency_names_are_exactly_the_frozen_set():
    """再单独钉一遍依赖「名字集合」，失败信息直接指出是谁被加进来 / 被拿掉。"""
    got = {d.split(">=")[0] for d in _pyproject()["project"]["dependencies"]}
    want = {d.split(">=")[0] for d in SPEC_30_PROJECT["dependencies"]}
    assert got == want, f"依赖集合跑偏：多出 {sorted(got - want)}，缺少 {sorted(want - got)}"


def test_pyproject_pytest_and_ruff_blocks_match_spec_30():
    """pytest / ruff 两段也在 §3.0 冻结范围内（asyncio_mode=auto 尤其关键，改掉异步测试会静默变皮壳）。"""
    tool = _pyproject()["tool"]
    assert tool["pytest"]["ini_options"] == SPEC_30_PYTEST_INI
    assert tool["ruff"] == SPEC_30_RUFF


def test_pyproject_build_system_is_frozen():
    """构建后端也冻住：换后端 / 加构建期依赖都要先过总管。

    §3.0 没写 [build-system]，这段是 T0 加的（回执里写明了理由）。既然加了，
    就不能是个没人看的活口 —— 悄悄塞一条 requires 等于绕开「新依赖要停下来报告」。"""
    assert _pyproject()["build-system"] == {
        "requires": ["setuptools>=68"],
        "build-backend": "setuptools.build_meta",
    }


def test_pyproject_has_no_undeclared_tool_blocks():
    """[tool.*] 里除了 §3.0 的 pytest/ruff 和那条写明理由的 setuptools，不许有别的段。"""
    extra = set(_pyproject()["tool"]) - {"pytest", "ruff"} - ALLOWED_EXTRA_TOOL_KEYS
    assert extra == set(), f"pyproject.toml 多出未声明的 [tool.*] 段：{sorted(extra)}"


# ---------------------------------------------------------------------------
# §3.1「从各子模块 re-export 全部公开类型（T0 写全 __all__）」—— 反向完整性
#
# 上面的 test_contracts_all_names_are_importable 只验了「__all__ 里的名字取得到」。
# 漏掉的另一半是「子模块里的公开定义有没有被漏写进 __all__」：漏写不会报错、
# 别的 track `from aite.contracts import *` 时才发现拿不到，那时已经跨轨了。
# 用 AST 读源码顶层定义（而不是 dir(module)），常量与类型别名也算，且天然排除 import 进来的名字。
# ---------------------------------------------------------------------------

CONTRACT_MODULE_FILES = [
    "capabilities.py", "config.py", "events.py", "evidence.py", "gateway.py",
    "outbound.py", "ports.py", "protocol.py", "sandbox.py", "session.py",
]


def _public_toplevel_names(rel_name: str) -> list[str]:
    """契约子模块源码里的顶层公开定义：class / def / 赋值常量 / 类型别名。"""
    tree = ast.parse((REPO_ROOT / "aite" / "contracts" / rel_name).read_text(encoding="utf-8"))
    names = []
    for node in tree.body:
        if isinstance(node, ast.ClassDef | ast.FunctionDef | ast.AsyncFunctionDef):
            names.append(node.name)
        elif isinstance(node, ast.Assign):
            names.extend(t.id for t in node.targets if isinstance(t, ast.Name))
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            names.append(node.target.id)
    return [n for n in names if not n.startswith("_")]


@pytest.mark.parametrize("rel_name", CONTRACT_MODULE_FILES)
def test_contracts_all_covers_every_public_definition(rel_name):
    """每个契约子模块的顶层公开定义都必须出现在包级 __all__ 里。"""
    declared = _public_toplevel_names(rel_name)
    assert declared, f"{rel_name} 没解析出任何公开定义，解析器八成坏了"
    missing = [n for n in declared if n not in contracts.__all__]
    assert missing == [], f"aite/contracts/{rel_name} 有公开定义没进 __all__：{missing}"


def test_contracts_all_is_exactly_submodule_surface_plus_version():
    """反过来：__all__ 里也不许有子模块之外凭空多出来的名字（CONTRACT_VERSION 除外）。"""
    surface = {"CONTRACT_VERSION"}
    for rel_name in CONTRACT_MODULE_FILES:
        surface.update(_public_toplevel_names(rel_name))
    assert set(contracts.__all__) == surface, (
        f"__all__ 与子模块公开面不等：多出 {sorted(set(contracts.__all__) - surface)}，"
        f"缺少 {sorted(surface - set(contracts.__all__))}"
    )


def test_contract_modules_on_disk_match_the_expected_file_list():
    """契约目录里的 .py 文件就是这 10 个（外加 __init__.py / lock.py）—— 新增契约文件必须显式过这条。"""
    on_disk = sorted(
        p.name for p in (REPO_ROOT / "aite" / "contracts").glob("*.py")
        if p.name not in ("__init__.py", "lock.py")
    )
    assert on_disk == sorted(CONTRACT_MODULE_FILES)


# ---------------------------------------------------------------------------
# §3.2 Protocol 上声明的「属性」（不是方法）：PlatformPort.capabilities / ModelPort.name
# ---------------------------------------------------------------------------

def test_contract_protocol_attributes_are_frozen():
    """契约里只有 PlatformPort.capabilities 和 ModelPort.name 两个属性声明，别的 Port 不许有。"""
    attrs = {
        f"{proto.__name__}.{n}"
        for proto, _expected in PORT_PROTOCOLS
        for n in getattr(proto, "__annotations__", {})
    }
    assert attrs == {"PlatformPort.capabilities", "ModelPort.name"}
    assert contract_ports.PlatformPort.__annotations__["capabilities"] is contracts.PlatformCapabilities
    assert contract_ports.ModelPort.__annotations__["name"] is str


# ---------------------------------------------------------------------------
# 包结构：aite/ 下每个装了 .py 的目录都得是真包
# ---------------------------------------------------------------------------

def test_every_aite_subdir_with_python_files_is_a_package():
    """中间层目录（如 aite/adapters）漏 __init__.py 会变隐式命名空间包 ——
    import 侥幸还能用，但 setuptools 的 packages.find 会漏打包，装出来的轮子少半个树。
    §3.4 的 SPEC_DIRS 只列了叶子目录，所以这里独立扫一遍全树。"""
    orphans = []
    for d in sorted((REPO_ROOT / "aite").rglob("*")):
        if not d.is_dir() or "__pycache__" in d.parts:
            continue
        if any(f.suffix == ".py" for f in d.iterdir()) and not (d / "__init__.py").is_file():
            orphans.append(d.relative_to(REPO_ROOT).as_posix())
    assert orphans == [], f"这些目录有 .py 但没有 __init__.py：{orphans}"
