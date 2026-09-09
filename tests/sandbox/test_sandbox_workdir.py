"""/work 路径规范化的纯单元测试（owner: T3）。

这一份不碰 Docker，但仍然被 conftest 打上 docker mark（本目录统一口径），
所以它跟着 B4 一起跑。规范化是 `put_file` / `get_file` 的第一道闸，
容器起不来时也该有人盯着它。
"""
import pytest

from aite.sandbox import (
    EXEC_TIMEOUT_EXIT_CODE,
    WORKDIR,
    SandboxPathError,
    normalize_work_path,
    require_file_path,
)


def test_workdir_is_slash_work():
    """§3.1 SandboxSpec.workdir 的默认值，别漂。"""
    assert WORKDIR == "/work"


def test_exec_timeout_exit_code_is_posix_124():
    """DockerSandbox 与 run_python 靠这个值对话，改了两边就对不上。"""
    assert EXEC_TIMEOUT_EXIT_CODE == 124


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        ("/work", "/work"),
        ("/work/out.png", "/work/out.png"),
        ("/work/in/data.csv", "/work/in/data.csv"),
        ("/work/./out.png", "/work/out.png"),
        ("/work/a/../b/./c.png", "/work/b/c.png"),
        ("/work//double//slash.txt", "/work/double/slash.txt"),
    ],
)
def test_paths_inside_work_are_normalized(raw, expected):
    assert str(normalize_work_path(raw)) == expected


@pytest.mark.parametrize(
    "raw",
    [
        "/etc/passwd",           # 压根不在 /work 下
        "/workspace/x",          # 前缀像但不是（不能用 startswith 判）
        "work/x",                # 相对路径
        "out.png",               # 相对路径
        "/work/../etc/passwd",   # 用 .. 爬出去
        "/work/a/../../etc/x",   # 多跳爬出去
        "/../work/x",            # 越过根
        "",                      # 空
        "   ",                   # 只有空白
    ],
)
def test_paths_outside_work_are_rejected(raw):
    with pytest.raises(SandboxPathError):
        normalize_work_path(raw)


def test_require_file_path_rejects_the_workdir_itself():
    """/work 是目录，put_file / get_file 拿它当文件是调用方的 bug。"""
    with pytest.raises(SandboxPathError):
        require_file_path(WORKDIR)
    assert str(require_file_path("/work/x")) == "/work/x"
