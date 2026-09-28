"""镜像里烘进去的四份运行时镜像源配置（owner: CC12）。

这几条只在 aite-sandbox 镜像里有意义：`--network none` 下它们不起作用，这里钉的是「文件在、
指向对的主机、属主是 aite」；主机清单与 CC11 的 china-trusted 预设逐主机对照。
"""

import configparser
import os
import tomllib
import unittest

PIP_CONF = "/etc/pip.conf"
NPMRC = "/home/aite/.npmrc"
CARGO_CONFIG = "/home/aite/.cargo/config.toml"
AITE_UID = 1000


def _read_text(path):
    try:
        with open(path, encoding="utf-8") as f:
            return f.read()
    except FileNotFoundError:
        return "<文件不存在>"


class ImageEnvTest(unittest.TestCase):
    def test_pip_conf_points_to_mainland_index(self):
        text = _read_text(PIP_CONF)
        cfg = configparser.ConfigParser()
        cfg.read_string(text if not text.startswith("<") else "")
        msg = f"{PIP_CONF} 的内容：\n{text}"
        self.assertEqual(cfg.get("global", "index-url", fallback=None), "https://pypi.tuna.tsinghua.edu.cn/simple", msg)
        self.assertEqual(cfg.get("global", "extra-index-url", fallback=None), "https://mirrors.aliyun.com/pypi/simple/", msg)

    def test_npmrc_points_to_npmmirror(self):
        text = _read_text(NPMRC)
        lines = [s.strip() for s in text.splitlines() if s.strip() and not s.lstrip().startswith(("#", ";"))]
        self.assertIn("registry=https://registry.npmmirror.com", lines, f"{NPMRC} 的内容：\n{text}")
        self.assertEqual(os.stat(NPMRC).st_uid, AITE_UID)

    def test_goproxy_env_is_goproxy_cn(self):
        self.assertEqual(os.environ.get("GOPROXY"), "https://goproxy.cn,direct")

    def test_cargo_config_points_to_ustc(self):
        text = _read_text(CARGO_CONFIG)
        cfg = tomllib.loads(text if not text.startswith("<") else "")
        msg = f"{CARGO_CONFIG} 的内容：\n{text}"
        source = cfg.get("source", {})
        self.assertEqual(source.get("crates-io", {}).get("replace-with"), "ustc-sparse", msg)
        self.assertEqual(
            source.get("ustc-sparse", {}).get("registry"),
            "sparse+https://mirrors.ustc.edu.cn/crates.io-index/",
            msg,
        )
        self.assertEqual(os.stat(CARGO_CONFIG).st_uid, AITE_UID)
        self.assertEqual(os.stat(os.path.dirname(CARGO_CONFIG)).st_uid, AITE_UID)

    def test_no_proxy_env_in_image(self):
        names = ("http_proxy", "https_proxy", "all_proxy", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY")
        present = {n: os.environ[n] for n in names if n in os.environ}
        self.assertEqual(present, {})


if __name__ == "__main__":
    unittest.main()
