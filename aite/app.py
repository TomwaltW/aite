"""进程入口（stub）。

组装 adapter / control plane / worker / gateway 归 TΩ（dev-spec §3.4、§5）。
T0 只保证这个模块 import 得过、main() 存在。
"""


def main() -> None:
    raise NotImplementedError("aite.app.main 由 TΩ 实现（dev-spec §3.4 归属表、§5 任务表）")


if __name__ == "__main__":
    main()
