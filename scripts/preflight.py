#!/usr/bin/env python3
"""起飞前自检 —— 把外部依赖挨个点名，一条命令看齐不齐（§2.4 的 M1–M6 之前跑）。

为什么要有它：M1–M6 是在真实飞书群里的人工验收，本来就难复现。凭证没设、
权限没批、模型端点不通、沙箱镜像不在 —— 这些今天全都要等 `python -m aite.app`
起来、在群里 @ 一句、然后看它怎么炸才知道，还分不清是环境没配好还是代码有问题。
这个脚本把那 60 秒挪到起飞之前：每项 OK/FAIL，外加一句人话说清怎么补。

七组检查（编号与派单一致）：

    1 配置可加载    load_config 不抛，打出 platform / model.provider / sandbox.image
    2 环境变量齐    config 里每个 *_env 字段点名的变量在不在（只报在不在）
    3 飞书凭证有效  tenant_access_token 换得到
    4 飞书身份对上  机器人自身 open_id 与 FEISHU_BOT_OPEN_ID 是不是同一个
    5 模型端点通    发一次最小 chat，报延迟 / token / 花费
    6 沙箱可用      daemon 连得上、镜像在、起容器跑通四个 import，跑完必须收掉
    7 落盘目录可写  sqlite_path / evidence_dir / artifacts_dir 落得下去

用法：

    python scripts/preflight.py                     # 七组全跑
    python scripts/preflight.py --offline           # 只跑 1/2/7（CI、没凭证的机器）
    python scripts/preflight.py --json              # 机器可读，给以后接 CI 用
    python scripts/preflight.py --chat-id oc_xxx    # 顺带核实 §3.7(b)：群历史读不读得到

退出码：任一 FAIL → 1，否则 0（WARN / SKIP 不算失败）。一项失败不阻断后面的检查，
全部跑完再汇总 —— 真机排障最烦的就是修一条重跑一次。

红线：**绝不打印任何密钥取值。** 第 2 组只报「已设置 / 未设置」，连前几位都不打；
第 3–5 组报的是通没通，不是拿到了什么。除了「不主动打」，还兜一层 `Redactor`：
所有要输出的文本在渲染前都过一遍替换，把已知取值（四个环境变量的值 +
换到的 tenant_access_token）换成占位符 —— 这样即使上游 API 把凭证回显进错误消息，
也漏不到 stdout/stderr 上。
"""
from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
import time
import unicodedata
import uuid
from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any

# 先把**本仓库根**顶到 sys.path 最前面，理由同 tests/conftest.py：几个 worktree 并行干活时，
# `python scripts/preflight.py` 只会把 scripts/ 塞进 sys.path，`import aite` 有可能解析到
# 别的树上去 —— 自检报的就成了别人的环境。仓库根有个 docker/ 目录，但它没有 __init__.py，
# 按 PEP 420 的规则「找得到真包就不用命名空间包」，`import docker` 仍然落在 SDK 上。
REPO_ROOT = Path(__file__).resolve().parent.parent
if sys.path[:1] != [str(REPO_ROOT)]:
    sys.path.insert(0, str(REPO_ROOT))

import httpx  # noqa: E402
from pydantic import BaseModel  # noqa: E402

from aite.adapters.feishu import DEFAULT_DOMAIN, FeishuApiClient, PlatformError  # noqa: E402
from aite.adapters.feishu.api import PATH_MESSAGES  # noqa: E402
from aite.config import load_config  # noqa: E402
from aite.contracts import ExecRequest, Message, SandboxSpec  # noqa: E402
from aite.contracts.config import AiteConfig, ModelConfig  # noqa: E402
from aite.models import ModelConfigError, OpenAICompatModel, cost_of, resolve_api_key  # noqa: E402
from aite.sandbox import SandboxError  # noqa: E402

#: 默认配置；没有实配时退到样例（形状一样，够跑 1/2/7）。
CONFIG_PATH = Path("config/aite.yaml")
EXAMPLE_CONFIG_PATH = Path("config/aite.example.yaml")

#: 「获取机器人信息」。附录 A 没列这条（它只列了 T1 要用的 im/docx/wiki 那几个），
#: 路径取自开放平台文档 open.feishu.cn 的 bot v3。它是 v3 老接口，响应形状是
#: {"code":0,"msg":"ok","bot":{...}} —— `bot` 在**顶层**而不是 `data` 里，
#: 而 `FeishuApiClient.request()` 按新接口的口径只回 `data`，所以这一发不走 request()，
#: 直接用同一个 httpx client 打，鉴权仍然用 `tenant_access_token()` 换来的 token。
PATH_BOT_INFO = "/open-apis/bot/v3/info"

#: 网络类检查的超时。起飞前自检要的是「快而诚实」，不是等它慢慢连上。
FEISHU_TIMEOUT_SEC = 20.0
MODEL_TIMEOUT_SEC = 60.0

#: 第 5 组发的最小 chat：够拿到一次 usage 就行，别烧钱。
MODEL_PROBE_PROMPT = "ping"
MODEL_PROBE_MAX_TOKENS = 16

#: 第 6 组在容器里跑的探针。四个 import 是 §6-T3 给镜像定的底线；
#: 顺手把版本号带回来，回执里贴的就是它。
SANDBOX_PROBE_CODE = """
import importlib, json
out = {}
for name in ("pandas", "matplotlib", "openpyxl", "docx"):
    out[name] = getattr(importlib.import_module(name), "__version__", "?")
print("AITE_PREFLIGHT_OK " + json.dumps(out))
"""
SANDBOX_PROBE_MARK = "AITE_PREFLIGHT_OK"
SANDBOX_PROBE_TIMEOUT_SEC = 60

#: 取值短于这个长度的不做脱敏替换 —— 那种东西本来也不是密钥，
#: 而拿一两个字符去全局 replace 会把正常输出打成马赛克。
MIN_REDACT_LEN = 4

STATUS_OK = "ok"
STATUS_FAIL = "fail"
STATUS_WARN = "warn"
STATUS_SKIP = "skip"

#: 汇总时的展示顺序与中文名。
_STATUS_LABEL = {STATUS_OK: "OK", STATUS_FAIL: "FAIL", STATUS_WARN: "WARN", STATUS_SKIP: "SKIP"}


# ---------------------------------------------------------------------------
# 结果模型
# ---------------------------------------------------------------------------

@dataclass
class CheckResult:
    """一组检查的结论。`fix` 只在非 OK 时有意义 —— 每条 FAIL 都要说清怎么补。"""

    name: str
    title: str
    status: str
    detail: str
    fix: str = ""
    extra: dict[str, Any] = field(default_factory=dict)

    @property
    def failed(self) -> bool:
        return self.status == STATUS_FAIL


@dataclass
class Note:
    """不进判据、只报事实的提示行（现在只有 §3.7 那两条待核实项）。"""

    name: str
    status: str          # verified | unverified | unverifiable
    detail: str


@dataclass
class Report:
    checks: list[CheckResult]
    notes: list[Note]
    config_path: str
    offline: bool

    @property
    def ok(self) -> bool:
        return not any(c.failed for c in self.checks)

    @property
    def passed(self) -> int:
        """「过了几项」= 没红的项。WARN / SKIP 不拦起飞，所以都算过。"""
        return sum(1 for c in self.checks if not c.failed)


# ---------------------------------------------------------------------------
# 脱敏
# ---------------------------------------------------------------------------

class Redactor:
    """把已知的密钥取值从任何要输出的文本里抹掉。

    这是「绝不打印密钥」的第二道闸：第一道是代码里根本不去打它们，
    但错误消息是上游给的（飞书的 msg、httpx 的 URL、SDK 的异常），
    谁也不能保证里面不带凭证。所有 detail / fix 在渲染前都过一遍这里。
    """

    def __init__(self) -> None:
        self._items: list[tuple[str, str]] = []

    def add(self, value: str | None, label: str) -> None:
        if value and len(value) >= MIN_REDACT_LEN and all(v != value for v, _ in self._items):
            self._items.append((value, label))
            # 长的先替换，免得短取值恰好是长取值的前缀时替出半截。
            self._items.sort(key=lambda item: len(item[0]), reverse=True)

    def scrub(self, text: str) -> str:
        for value, label in self._items:
            text = text.replace(value, f"«{label} 的取值已隐去»")
        return text

    def scrub_obj(self, obj: Any) -> Any:
        """递归洗 `extra` 这种嵌套结构。

        不走「json.dumps → 字符串替换 → json.loads」那条近路：取值里只要有引号或反斜杠，
        dumps 会把它转义掉，字符串替换就对不上，脱敏静默失效。
        """
        if isinstance(obj, str):
            return self.scrub(obj)
        if isinstance(obj, dict):
            return {k: self.scrub_obj(v) for k, v in obj.items()}
        if isinstance(obj, list):
            return [self.scrub_obj(v) for v in obj]
        return obj


# ---------------------------------------------------------------------------
# 小工具
# ---------------------------------------------------------------------------

def _resolve(path: str | Path) -> Path:
    """相对路径一律按仓库根解析 —— 和 scripts/check.sh 的 `cd $(dirname $0)/..` 一个意思，
    但不改进程的工作目录（那是会传染给被检查对象的副作用）。"""
    p = Path(path)
    return p if p.is_absolute() else REPO_ROOT / p


def _rel(path: Path) -> str:
    """仓库内的路径显示成相对路径 —— 自检行是给人扫的，绝对路径把行撑爆了。"""
    try:
        return str(path.relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


def _display_width(text: str) -> int:
    return sum(2 if unicodedata.east_asian_width(ch) in "WF" else 1 for ch in text)


def _pad(text: str, width: int) -> str:
    return text + " " * max(0, width - _display_width(text))


def _tail(text: str, limit: int = 300) -> str:
    """错误输出只留尾巴 —— 自检行是给人一眼扫的，完整栈去看日志。"""
    text = " ".join(text.split())
    return text if len(text) <= limit else "…" + text[-limit:]


def env_var_names(cfg: AiteConfig) -> list[tuple[str, str]]:
    """扫出 config 里所有 `*_env` 字段 → [(字段路径, 环境变量名)]。

    generic 地扫而不是写死那四个名字：契约以后再加一个 `*_env`，这里自动跟上，
    不会出现「新加的凭证自检查不到」这种静默盲区。
    """
    out: list[tuple[str, str]] = []

    def walk(model: BaseModel, prefix: str) -> None:
        for fname in type(model).model_fields:
            value = getattr(model, fname)
            if isinstance(value, BaseModel):
                walk(value, f"{prefix}{fname}.")
            elif fname.endswith("_env") and isinstance(value, str) and value:
                out.append((f"{prefix}{fname}", value))

    walk(cfg, "")
    return out


# ---------------------------------------------------------------------------
# 1 配置可加载
# ---------------------------------------------------------------------------

def check_config(path: Path, *, fell_back: bool) -> tuple[CheckResult, AiteConfig | None]:
    try:
        cfg = load_config(path)
    except Exception as exc:                        # noqa: BLE001 - 自检要把任何异常翻成一行结论
        return CheckResult(
            name="config",
            title="配置可加载",
            status=STATUS_FAIL,
            detail=f"{type(exc).__name__}: {_tail(str(exc))}",
            fix=f"照 {EXAMPLE_CONFIG_PATH} 的形状修 {path}；密钥只写环境变量名，不写取值",
        ), None

    detail = (
        f"platform={cfg.platform} · model.provider={cfg.model.provider} "
        f"· sandbox.image={cfg.sandbox.image}"
    )
    extra = {
        "platform": cfg.platform,
        "model_provider": cfg.model.provider,
        "sandbox_image": cfg.sandbox.image,
        "config_path": str(path),
    }
    if fell_back:
        # 样例配置能过形状校验，但 base_url / model 是空的，真机起飞用它必炸。
        # 不算 FAIL（CI 和这台机器上本来就只有样例），但必须显式说出来。
        return CheckResult(
            name="config",
            title="配置可加载",
            status=STATUS_WARN,
            detail=f"{CONFIG_PATH} 不存在，退到样例 {EXAMPLE_CONFIG_PATH} · {detail}",
            fix=f"起飞前 `cp {EXAMPLE_CONFIG_PATH} {CONFIG_PATH}` 并填上 model.base_url / model.model",
            extra=extra | {"fell_back_to_example": True},
        ), cfg

    return CheckResult(
        name="config", title="配置可加载", status=STATUS_OK, detail=detail, extra=extra
    ), cfg


# ---------------------------------------------------------------------------
# 2 环境变量齐
# ---------------------------------------------------------------------------

def check_env(cfg: AiteConfig, env: Mapping[str, str], *, offline: bool) -> CheckResult:
    """只报「在不在」。取值一个字都不出现在这里 —— 派单的红线第一条。"""
    names = env_var_names(cfg)
    missing = [var for _field, var in names if not (env.get(var) or "").strip()]
    present = [var for _field, var in names if (env.get(var) or "").strip()]
    extra = {"required": [var for _f, var in names], "missing": missing, "present": present}

    if not missing:
        return CheckResult(
            name="env",
            title="环境变量齐",
            status=STATUS_OK,
            detail=f"{len(names)} 个都已设置：{' '.join(present)}",
            extra=extra,
        )

    detail = f"{len(missing)}/{len(names)} 个未设置：{' '.join(missing)}"
    if present:
        detail += f"（已设置：{' '.join(present)}）"
    fix = (
        "export " + " ".join(f"{var}=…" for var in missing)
        + "；飞书三项去开放平台 → 应用 → 凭证与基础信息 / 机器人，模型密钥去模型厂商控制台。"
        "取值只放环境变量，不要写进 config/aite.yaml"
    )
    if offline:
        # `--offline` 就是给 CI 和没凭证的机器用的，在这儿把缺变量判成 FAIL
        # 等于说「CI 上永远跑不过」。降成 WARN，但一个名字都不少报。
        return CheckResult(
            name="env",
            title="环境变量齐",
            status=STATUS_WARN,
            detail=detail + " —— --offline 下不作判据",
            fix="真机起飞前去掉 --offline 重跑一次，这几项必须是 OK",
            extra=extra | {"offline_downgraded": True},
        )
    return CheckResult(
        name="env", title="环境变量齐", status=STATUS_FAIL, detail=detail, fix=fix, extra=extra
    )


# ---------------------------------------------------------------------------
# 3 / 4 飞书：凭证有效 + 身份对得上（顺带 §3.7(b) 的探测）
# ---------------------------------------------------------------------------

def _skipped(name: str, title: str, reason: str) -> CheckResult:
    return CheckResult(name=name, title=title, status=STATUS_SKIP, detail=reason)


def _crashed(name: str, title: str, exc: BaseException) -> CheckResult:
    """自检这一项自己炸了。

    这条存在的意义：「一项失败不阻断后面的检查」要对**没预料到的**失败也成立。
    真机排障时最不能接受的就是 preflight 自己抛栈退出，剩下六项一个都没跑。
    """
    return CheckResult(
        name=name,
        title=title,
        status=STATUS_FAIL,
        detail=f"这一项自己炸了：{type(exc).__name__}: {_tail(str(exc))}",
        fix="把这行连同上面的输出一起报告 —— 它说明 preflight 漏了一种失败形态，不是环境的问题",
    )


def _blocked(name: str, title: str, missing: list[str]) -> CheckResult:
    return CheckResult(
        name=name,
        title=title,
        status=STATUS_FAIL,
        detail=f"前置未满足：{' '.join(missing)} 未设置，这一项没法查",
        fix="先把第 2 组点名的环境变量补齐再重跑",
    )


async def check_feishu(
    cfg: AiteConfig,
    env: Mapping[str, str],
    redactor: Redactor,
    *,
    chat_id: str | None,
    http_client: httpx.AsyncClient | None = None,
    domain: str = DEFAULT_DOMAIN,
) -> tuple[CheckResult, CheckResult, list[Note]]:
    """第 3 组（换 token）和第 4 组（身份比对）共用一条连接，一起跑。

    第 4 组为什么重要：app_id/app_secret 和 FEISHU_BOT_OPEN_ID 来自不同应用时，
    token 照样换得到、进程照样起得来，但 @ 识别永远匹配不上 —— M1 表现为「静默不响应」，
    在群里看是最难查的一种炸法。
    """
    app_id = (env.get(cfg.feishu.app_id_env) or "").strip()
    app_secret = (env.get(cfg.feishu.app_secret_env) or "").strip()
    want_open_id = (env.get(cfg.feishu.bot_open_id_env) or "").strip()

    redactor.add(app_id, cfg.feishu.app_id_env)
    redactor.add(app_secret, cfg.feishu.app_secret_env)
    redactor.add(want_open_id, cfg.feishu.bot_open_id_env)

    missing = [
        name
        for name, value in ((cfg.feishu.app_id_env, app_id), (cfg.feishu.app_secret_env, app_secret))
        if not value
    ]
    if missing:
        blocked = _blocked("feishu_token", "飞书凭证有效", missing)
        blocked_id = _blocked("feishu_identity", "飞书身份对得上", missing)
        return blocked, blocked_id, [_note_passive_listen(), _note_history_scope_unverified()]

    owns_client = http_client is None
    # 注入自己的 httpx client：第 4 组的 bot/v3/info 拿不到 `data`（见 PATH_BOT_INFO 的注释），
    # 得直接读原始响应体。鉴权、退避重试仍然走 FeishuApiClient。
    client = http_client or httpx.AsyncClient(timeout=FEISHU_TIMEOUT_SEC)
    api = FeishuApiClient(app_id=app_id, app_secret=app_secret, domain=domain, client=client)

    notes: list[Note] = [_note_passive_listen()]
    try:
        token_result, token = await _check_feishu_token(api, redactor)
        if token is None:
            identity = _skipped("feishu_identity", "飞书身份对得上", "第 3 组没过，身份无从比对")
            notes.append(_note_history_scope_unverified())
            return token_result, identity, notes

        identity = await _check_feishu_identity(client, token, want_open_id, cfg, domain=domain)
        notes.append(
            await _probe_history_scope(api, chat_id) if chat_id else _note_history_scope_unverified()
        )
        return token_result, identity, notes
    finally:
        # aclose() 在注入了 client 的情况下是空转（_owns_client=False），但收尾姿态照做；
        # 传输层是我建的就由我关，不给事件循环留下未关闭的连接。
        await api.aclose()
        if owns_client:
            await client.aclose()


async def _check_feishu_token(
    api: FeishuApiClient, redactor: Redactor
) -> tuple[CheckResult, str | None]:
    started = time.perf_counter()
    try:
        token = await api.tenant_access_token()
    except PlatformError as exc:
        return CheckResult(
            name="feishu_token",
            title="飞书凭证有效",
            status=STATUS_FAIL,
            detail=f"换 tenant_access_token 失败：code={exc.code} msg={_tail(exc.message)}",
            fix="核对 FEISHU_APP_ID / FEISHU_APP_SECRET 是不是同一个自建应用的凭证"
                "（开放平台 → 应用 → 凭证与基础信息）；连不上则先看本机出网",
        ), None
    except Exception as exc:                        # noqa: BLE001
        return CheckResult(
            name="feishu_token",
            title="飞书凭证有效",
            status=STATUS_FAIL,
            detail=f"换 tenant_access_token 失败：{type(exc).__name__}: {_tail(str(exc))}",
            fix="先确认本机能出网访问 open.feishu.cn，再核对两个凭证环境变量",
        ), None

    # token 本身是密钥，进脱敏表；后面任何错误消息里再出现它都会被抹掉。
    redactor.add(token, "tenant_access_token")
    elapsed = int((time.perf_counter() - started) * 1000)
    return CheckResult(
        name="feishu_token",
        title="飞书凭证有效",
        status=STATUS_OK,
        detail=f"tenant_access_token 换到了（{elapsed}ms，取值不打印）",
        extra={"latency_ms": elapsed},
    ), token


async def _check_feishu_identity(
    client: httpx.AsyncClient,
    token: str,
    want_open_id: str,
    cfg: AiteConfig,
    *,
    domain: str,
) -> CheckResult:
    try:
        resp = await client.get(
            domain.rstrip("/") + PATH_BOT_INFO, headers={"Authorization": f"Bearer {token}"}
        )
        body = resp.json() if resp.content else {}
        body = body if isinstance(body, dict) else {}
    except Exception as exc:                        # noqa: BLE001
        return CheckResult(
            name="feishu_identity",
            title="飞书身份对得上",
            status=STATUS_FAIL,
            detail=f"查机器人信息失败：{type(exc).__name__}: {_tail(str(exc))}",
            fix=f"确认应用开了机器人能力；接口 GET {PATH_BOT_INFO}",
        )

    code = body.get("code", resp.status_code)
    if code != 0:
        return CheckResult(
            name="feishu_identity",
            title="飞书身份对得上",
            status=STATUS_FAIL,
            detail=f"查机器人信息失败：http={resp.status_code} code={code} msg={_tail(str(body.get('msg', '')))}",
            fix="多半是应用没开机器人能力，或权限没批 —— 对照 §3.7 的权限清单："
                "接收群聊中 @ 机器人消息 / 读取群历史消息 / 发送与更新消息与卡片 / 上传下载文件 / 消息表情回复",
        )

    bot = body.get("bot") or {}
    bot = bot if isinstance(bot, dict) else {}
    got_open_id = str(bot.get("open_id") or "")
    app_name = str(bot.get("app_name") or "?")
    # activate_status 原样带出，不做数字→含义的解释：开放平台的取值表我没在这轮核实过，
    # 猜一个映射写进自检输出，比不写更害人。
    activate = bot.get("activate_status")
    extra = {"app_name": app_name, "activate_status": activate, "bot_open_id_matches": None}

    if not got_open_id:
        return CheckResult(
            name="feishu_identity",
            title="飞书身份对得上",
            status=STATUS_FAIL,
            detail=f"响应里没有 bot.open_id（app_name={app_name}）",
            fix=f"确认应用开了机器人能力；接口 GET {PATH_BOT_INFO} 的响应应含 bot.open_id",
            extra=extra,
        )
    if not want_open_id:
        return CheckResult(
            name="feishu_identity",
            title="飞书身份对得上",
            status=STATUS_FAIL,
            detail=f"飞书侧机器人取到了（app_name={app_name}），但 {cfg.feishu.bot_open_id_env} 未设置，没法比对",
            fix=f"export {cfg.feishu.bot_open_id_env}=<开放平台 → 应用 → 机器人 页面上该机器人的 open_id>"
                "；缺了它 @ 识别永远匹配不上，M1 会静默不响应",
            extra=extra,
        )
    if got_open_id != want_open_id:
        # 两个取值一个都不打 —— 红线在这儿最容易破，因为「打出来才好查」的诱惑最大。
        return CheckResult(
            name="feishu_identity",
            title="飞书身份对得上",
            status=STATUS_FAIL,
            detail=f"对不上：飞书侧机器人是 {app_name}，与 {cfg.feishu.bot_open_id_env} 不是同一个（取值都不打印）",
            fix=f"{cfg.feishu.app_id_env}/{cfg.feishu.app_secret_env} 与 {cfg.feishu.bot_open_id_env} "
                "多半来自两个不同的应用；到开放平台上认准同一个应用，重取凭证和机器人 open_id",
            extra=extra | {"bot_open_id_matches": False},
        )

    return CheckResult(
        name="feishu_identity",
        title="飞书身份对得上",
        status=STATUS_OK,
        detail=f"一致 · app_name={app_name} · activate_status={activate}（取值含义见开放平台文档，此处不解释）",
        extra=extra | {"bot_open_id_matches": True},
    )


def _note_passive_listen() -> Note:
    """§3.7(a)。这条**没法在起飞前查**：能不能收到不带 @ 的话题回复，
    只有真的在话题里发一条、看事件有没有投递才知道 —— M4 本身就是那个实验。"""
    return Note(
        name="feishu_passive_listen",
        status="unverifiable",
        detail="§3.7(a) 只有 @ 权限时话题内不带 @ 的回复是否投递 —— 未核实，且起飞前查不了："
               "要真在话题里发一条不带 @ 的消息、看事件有没有投递才知道，M4 就是那个实验。"
               "当前 FEISHU_P0.supports_passive_listen=False（保守取值），M4 请带 @ 先走通。",
    )


def _note_history_scope_unverified() -> Note:
    return Note(
        name="feishu_group_history_scope",
        status="unverified",
        detail="§3.7(b) 群历史是否要「获取群组中所有消息」敏感权限 —— 未核实。"
               "飞书没有「列出本应用已授权范围」的免权限接口，光靠凭证问不出来；"
               "带 --chat-id <测试群 chat_id> 重跑，我就直接调一次群历史给你结论（M5 靠它）。",
    )


async def _probe_history_scope(api: FeishuApiClient, chat_id: str) -> Note:
    """§3.7(b) 的实测：拿真 chat_id 调一次群历史，读得到就是读得到。

    比猜错误码可靠 —— 这里问的本来就不是「有没有某个 scope」，而是
    「M5 要的群历史，现在这套凭证读不读得到」。
    """
    try:
        data = await api.request(
            "GET",
            PATH_MESSAGES,
            params={"container_id_type": "chat", "container_id": chat_id, "page_size": 1},
        )
    except PlatformError as exc:
        return Note(
            name="feishu_group_history_scope",
            status="verified",
            detail=f"§3.7(b) 实测：这套凭证读**不到**群历史（code={exc.code} msg={_tail(exc.message)}）。"
                   "去开放平台补权限（读取群历史消息；若提示需要「获取群组中所有消息」则它就是必需的敏感权限，"
                   "要走审核），补完重跑本项。M5「汇总本群本周开放事项」在此之前一定过不了。",
        )
    except Exception as exc:                        # noqa: BLE001
        return Note(
            name="feishu_group_history_scope",
            status="unverified",
            detail=f"§3.7(b) 探测没跑成：{type(exc).__name__}: {_tail(str(exc))}",
        )

    items = (data or {}).get("items") or []
    return Note(
        name="feishu_group_history_scope",
        status="verified",
        detail=f"§3.7(b) 实测：这套凭证**读得到**群历史（chat 返回 {len(items)} 条，只取了 1 页 1 条）。"
               "也就是说当前已授予的权限足够 M5；至于是不是「获取群组中所有消息」那条在起作用，"
               "接口不回权限来源，问不出来 —— 但对起飞而言结论已经够用。",
    )


# ---------------------------------------------------------------------------
# 5 模型端点通
# ---------------------------------------------------------------------------

async def check_model(
    cfg: AiteConfig,
    env: Mapping[str, str],
    redactor: Redactor,
    *,
    client_factory: Callable[[ModelConfig, str], Any] | None = None,
) -> CheckResult:
    mcfg = cfg.model
    title = "模型端点通"
    redactor.add((env.get(mcfg.api_key_env) or "").strip(), mcfg.api_key_env)

    if mcfg.provider != "openai_compat":
        return CheckResult(
            name="model",
            title=title,
            status=STATUS_WARN,
            detail=f"model.provider={mcfg.provider}，不是 openai_compat，没有真端点可探",
            fix="真机起飞要把 model.provider 设回 openai_compat",
        )

    # 缺什么一次报齐，别让人补完 base_url 重跑一遍才发现还缺 key。
    blanks = [f"model.{f}" for f in ("base_url", "model") if not getattr(mcfg, f)]
    api_key = ""
    try:
        api_key = resolve_api_key(mcfg, dict(env))
    except ModelConfigError:
        blanks.append(f"环境变量 {mcfg.api_key_env}")
    if blanks:
        fixes = []
        if not mcfg.base_url:
            fixes.append("model.base_url 填百炼 / 智谱的 OpenAI 兼容端点")
        if not mcfg.model:
            fixes.append("model.model 填模型名")
        if not api_key:
            fixes.append(f"export {mcfg.api_key_env}=<模型厂商控制台里的 API Key>（只放环境变量）")
        return CheckResult(
            name="model",
            title=title,
            status=STATUS_FAIL,
            detail=f"配置不全，缺：{' / '.join(blanks)}",
            fix="；".join(fixes),
            extra={"missing": blanks},
        )
    redactor.add(api_key, mcfg.api_key_env)

    factory = client_factory or _default_model_client
    client = factory(mcfg, api_key)
    model = OpenAICompatModel(mcfg, client=client)
    started = time.perf_counter()
    try:
        turn = await asyncio.wait_for(
            model.chat(
                [Message(role="user", content=MODEL_PROBE_PROMPT)],
                [],
                max_tokens=MODEL_PROBE_MAX_TOKENS,
                temperature=mcfg.temperature,
            ),
            timeout=MODEL_TIMEOUT_SEC,
        )
    except TimeoutError:
        return CheckResult(
            name="model",
            title=title,
            status=STATUS_FAIL,
            detail=f"{MODEL_TIMEOUT_SEC:.0f}s 内没回 —— 端点不通或太慢",
            fix=f"核对 model.base_url（现在是 {mcfg.base_url}）能不能从本机访问，以及是否需要走代理",
        )
    except Exception as exc:                        # noqa: BLE001
        return CheckResult(
            name="model",
            title=title,
            status=STATUS_FAIL,
            detail=f"{type(exc).__name__}: {_tail(str(exc))}",
            fix=f"401/403 → 换 {mcfg.api_key_env} 的取值；404 → 核对 model.base_url 结尾是否要带 /v1、"
                f"model.model={mcfg.model} 这个模型名在该厂商是否存在",
        )
    finally:
        await _aclose_quiet(client)

    elapsed = int((time.perf_counter() - started) * 1000)
    usage = turn.usage
    cost = cost_of(usage, mcfg)
    priced = mcfg.price_in_per_mtok or mcfg.price_out_per_mtok
    money = f"¥{cost:.6f}" if priced else "¥0（config 里 price_in/out_per_mtok 都是 0，没配价格）"
    return CheckResult(
        name="model",
        title=title,
        status=STATUS_OK,
        detail=f"{mcfg.model} 通了 · {elapsed}ms · in={usage.input_tokens} out={usage.output_tokens} "
               f"tokens · 本次 {money} · finish={turn.finish_reason}",
        extra={
            "latency_ms": elapsed,
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "cached_tokens": usage.cached_tokens,
            # 9 位而不是显示用的 6 位：一次最小 chat 只花几微元，6 位一舍就没了。
            "cost_cny": round(cost, 9),
            "finish_reason": turn.finish_reason,
        },
    )


def _default_model_client(mcfg: ModelConfig, api_key: str) -> Any:
    """自己建 AsyncOpenAI 而不是让 OpenAICompatModel 懒建：自检要能设超时、
    要能关掉重试（起飞前要的是一次诚实往返，不是 SDK 帮你重试到看起来还行），
    而且建出来的对象要拿在手上，跑完能关掉。"""
    from openai import AsyncOpenAI

    return AsyncOpenAI(
        base_url=mcfg.base_url, api_key=api_key, timeout=MODEL_TIMEOUT_SEC, max_retries=0
    )


async def _aclose_quiet(client: Any) -> None:
    closer = getattr(client, "close", None)
    if closer is None:
        return
    try:
        result = closer()
        if asyncio.iscoroutine(result):
            await result
    except Exception:                               # noqa: BLE001 - 关不掉不该盖掉真正的结论
        pass


# ---------------------------------------------------------------------------
# 6 沙箱可用
# ---------------------------------------------------------------------------

async def check_sandbox(
    cfg: AiteConfig, *, docker_client_factory: Callable[[], Any] | None = None
) -> CheckResult:
    """daemon → 镜像 → 起容器 → 跑四个 import → **一定收掉**。

    收容器这件事由 `finally` 里的 `release()` + `aclose()` 兜底，两者都幂等；
    最后再按 `aite.task` 标签回扫一遍，确认这次自检没在本机留下任何容器 ——
    自检本身要是漏了容器，比它检出来的问题还讨厌。
    """
    title = "沙箱可用"
    from docker.client import DockerClient
    from docker.errors import DockerException, ImageNotFound

    from aite.sandbox.docker_sandbox import LABEL_TASK, DockerSandbox

    factory = docker_client_factory or DockerClient.from_env
    try:
        client = await asyncio.to_thread(factory)
    except DockerException as exc:
        return CheckResult(
            name="sandbox",
            title=title,
            status=STATUS_FAIL,
            detail=f"连不上 Docker daemon：{_tail(str(exc))}",
            fix="启动 Docker Desktop（或 `colima start`），`docker info` 能出东西再重跑",
        )

    task_id = f"preflight-{uuid.uuid4().hex[:8]}"
    extra: dict[str, Any] = {"task_id": task_id, "image": cfg.sandbox.image}
    sandbox = DockerSandbox(client=client)
    sandbox_id: str | None = None
    try:
        try:
            version = await asyncio.to_thread(client.version)
            extra["docker_version"] = (version or {}).get("Version")
        except DockerException as exc:
            return CheckResult(
                name="sandbox",
                title=title,
                status=STATUS_FAIL,
                detail=f"Docker daemon 应答不了：{_tail(str(exc))}",
                fix="`docker info` 看 daemon 状态；Docker Desktop 刚起来时要等它就绪",
            )

        try:
            image = await asyncio.to_thread(client.images.get, cfg.sandbox.image)
            extra["image_id"] = str(getattr(image, "id", ""))[:19]
        except ImageNotFound:
            return CheckResult(
                name="sandbox",
                title=title,
                status=STATUS_FAIL,
                detail=f"本机没有镜像 {cfg.sandbox.image}",
                fix=f"docker build -t {cfg.sandbox.image} docker/sandbox",
            )
        except DockerException as exc:
            return CheckResult(
                name="sandbox",
                title=title,
                status=STATUS_FAIL,
                detail=f"查镜像 {cfg.sandbox.image} 失败：{_tail(str(exc))}",
                fix="`docker images` 看看 daemon 那边正不正常",
            )

        spec = SandboxSpec(image=cfg.sandbox.image, cpu=cfg.sandbox.cpu, mem_mb=cfg.sandbox.mem_mb)
        started = time.perf_counter()
        try:
            sandbox_id = await sandbox.acquire(task_id, spec)
        except SandboxError as exc:
            return CheckResult(
                name="sandbox",
                title=title,
                status=STATUS_FAIL,
                detail=f"起不了容器：{_tail(str(exc))}",
                fix=f"docker build -t {cfg.sandbox.image} docker/sandbox 重建镜像；"
                    "镜像里必须有 coreutils 的 timeout，且 /work 可写",
            )

        try:
            result = await sandbox.exec(
                sandbox_id,
                ExecRequest(
                    code=SANDBOX_PROBE_CODE,
                    timeout_sec=min(cfg.sandbox.exec_timeout_sec, SANDBOX_PROBE_TIMEOUT_SEC),
                ),
            )
        except SandboxError as exc:
            return CheckResult(
                name="sandbox",
                title=title,
                status=STATUS_FAIL,
                detail=f"容器起来了但探针没跑成：{_tail(str(exc))}",
                fix="`docker logs` 看容器还在不在；镜像里的 python 要能跑 `-c` 脚本",
                extra=extra,
            )
        elapsed = int((time.perf_counter() - started) * 1000)
        extra["elapsed_ms"] = elapsed
        if result.exit_code != 0 or SANDBOX_PROBE_MARK not in result.stdout:
            return CheckResult(
                name="sandbox",
                title=title,
                status=STATUS_FAIL,
                detail=f"容器起来了但四个 import 没跑通（exit={result.exit_code}）："
                       f"{_tail(result.stderr or result.stdout)}",
                fix=f"镜像缺包。重建：docker build -t {cfg.sandbox.image} docker/sandbox；"
                    "镜像内 `python -c \"import pandas, matplotlib, openpyxl, docx\"` 要能过",
                extra=extra,
            )
        versions = _parse_probe(result.stdout)
        extra["packages"] = versions
        detail = (
            f"{cfg.sandbox.image} 起容器 + 四个 import 跑通（{elapsed}ms）· "
            + " ".join(f"{k}={v}" for k, v in versions.items())
        )
    finally:
        # 异常路径也走这里：release 幂等，aclose 再把本进程记着的全部沙箱收一遍并关掉客户端。
        if sandbox_id is not None:
            try:
                await sandbox.release(sandbox_id)
            except SandboxError:
                pass
        try:
            await sandbox.aclose()
        except Exception:                           # noqa: BLE001
            pass

    leftover = await _leftover_containers(client, LABEL_TASK, task_id)
    extra["leftover_containers"] = leftover
    if leftover:
        return CheckResult(
            name="sandbox",
            title=title,
            status=STATUS_FAIL,
            detail=detail + f" —— 但自检的容器没收干净，还剩 {len(leftover)} 个",
            fix=f"docker rm -f $(docker ps -aq --filter label={LABEL_TASK}={task_id})，"
                "然后报告这个现象：收尾路径漏了",
            extra=extra,
        )
    return CheckResult(
        name="sandbox",
        title=title,
        status=STATUS_OK,
        detail=detail + " · 容器已收干净",
        extra=extra,
    )


def _parse_probe(stdout: str) -> dict[str, str]:
    for line in stdout.splitlines():
        if line.startswith(SANDBOX_PROBE_MARK):
            try:
                return json.loads(line[len(SANDBOX_PROBE_MARK):].strip())
            except json.JSONDecodeError:
                return {}
    return {}


async def _leftover_containers(client: Any, label_key: str, task_id: str) -> list[str]:
    """按标签回扫，确认这次自检没留下容器。查不了就当作没留（不拿基础设施的抖动去红一条真结论）。"""
    try:
        containers = await asyncio.to_thread(
            client.containers.list, all=True, filters={"label": f"{label_key}={task_id}"}
        )
    except Exception:                               # noqa: BLE001
        return []
    return [str(getattr(c, "id", ""))[:12] for c in containers]


# ---------------------------------------------------------------------------
# 7 落盘目录可写
# ---------------------------------------------------------------------------

def check_storage(cfg: AiteConfig) -> CheckResult:
    """判据是「落得下去」而不是「目录已经在」。

    `EvidenceWriter` 和 SQLite store 都是 `mkdir(parents=True, exist_ok=True)` 自建目录的，
    所以真正会让起飞炸掉的是「最近的那层已存在祖先写不了」，不是「data/ 还没建」。
    """
    targets = [
        ("storage.sqlite_path", cfg.storage.sqlite_path),
        ("storage.evidence_dir", cfg.storage.evidence_dir),
        ("storage.artifacts_dir", cfg.storage.artifacts_dir),
    ]
    bad: list[str] = []
    todo: list[str] = []
    extra: dict[str, Any] = {}
    for field_name, raw in targets:
        parent = _resolve(raw).parent
        anchor = parent
        while not anchor.exists() and anchor != anchor.parent:
            anchor = anchor.parent
        writable = anchor.is_dir() and os.access(anchor, os.W_OK | os.X_OK)
        extra[field_name] = {
            "path": raw,
            "parent": _rel(parent),
            "existing_ancestor": _rel(anchor),
            "writable": writable,
        }
        if not writable:
            bad.append(f"{raw}（卡在 {_rel(anchor)}）")
        elif not parent.exists() and _rel(parent) not in todo:
            todo.append(_rel(parent))

    paths = " · ".join(raw for _f, raw in targets)
    if bad:
        return CheckResult(
            name="storage",
            title="落盘目录可写",
            status=STATUS_FAIL,
            detail="写不下去：" + "；".join(bad),
            fix="给这几层目录写权限，或把 config 里的 storage.* 指到一个可写的位置",
            extra=extra,
        )
    pending = f"（{' '.join(todo)} 待建，起飞时自动 mkdir）" if todo else ""
    return CheckResult(
        name="storage",
        title="落盘目录可写",
        status=STATUS_OK,
        detail=f"3 个路径都落得下去{pending}：{paths}",
        extra=extra,
    )


# ---------------------------------------------------------------------------
# 编排
# ---------------------------------------------------------------------------

@dataclass
class Deps:
    """外部依赖的入口。默认全是真家伙；测试从这里打桩，所以单测既不连网也不起容器。"""

    http_client: httpx.AsyncClient | None = None
    model_client_factory: Callable[[ModelConfig, str], Any] | None = None
    docker_client_factory: Callable[[], Any] | None = None
    feishu_domain: str = DEFAULT_DOMAIN


async def run_checks(
    *,
    config_path: Path,
    fell_back: bool,
    env: Mapping[str, str],
    offline: bool,
    chat_id: str | None,
    redactor: Redactor,
    deps: Deps,
    display_path: str | None = None,
) -> Report:
    """七组依次跑完再汇总 —— 一项失败绝不阻断后面的。

    每一项都套 `_crashed`：预料内的失败各自有 fix，预料外的异常也只红这一行，
    绝不让 preflight 自己抛栈退出、把剩下几项一起带走。
    """
    shown = display_path or str(config_path)
    notes: list[Note] = []
    try:
        cfg_result, cfg = check_config(config_path, fell_back=fell_back)
    except Exception as exc:                        # noqa: BLE001
        cfg_result, cfg = _crashed("config", "配置可加载", exc), None
    checks: list[CheckResult] = [cfg_result]

    if cfg is None:
        # 配置都读不出来，剩下六项的判据无从谈起；照样各占一行，说清为什么。
        for name, title in (
            ("env", "环境变量齐"),
            ("feishu_token", "飞书凭证有效"),
            ("feishu_identity", "飞书身份对得上"),
            ("model", "模型端点通"),
            ("sandbox", "沙箱可用"),
            ("storage", "落盘目录可写"),
        ):
            checks.append(_skipped(name, title, "第 1 组没过，配置读不出来"))
        return Report(checks=checks, notes=notes, config_path=shown, offline=offline)

    checks.append(_guard("env", "环境变量齐", lambda: check_env(cfg, env, offline=offline)))

    if offline:
        reason = "--offline：不碰网络"
        checks.append(_skipped("feishu_token", "飞书凭证有效", reason))
        checks.append(_skipped("feishu_identity", "飞书身份对得上", reason))
        notes.append(_note_passive_listen())
        notes.append(_note_history_scope_unverified())
        checks.append(_skipped("model", "模型端点通", reason))
        checks.append(_skipped("sandbox", "沙箱可用", "--offline：不碰 docker"))
    else:
        try:
            token_result, identity_result, feishu_notes = await check_feishu(
                cfg, env, redactor,
                chat_id=chat_id, http_client=deps.http_client, domain=deps.feishu_domain,
            )
        except Exception as exc:                    # noqa: BLE001
            token_result = _crashed("feishu_token", "飞书凭证有效", exc)
            identity_result = _skipped("feishu_identity", "飞书身份对得上", "第 3 组自己炸了")
            feishu_notes = [_note_passive_listen(), _note_history_scope_unverified()]
        checks.append(token_result)
        checks.append(identity_result)
        notes.extend(feishu_notes)
        checks.append(await _guard_async(
            "model", "模型端点通",
            lambda: check_model(cfg, env, redactor, client_factory=deps.model_client_factory),
        ))
        checks.append(await _guard_async(
            "sandbox", "沙箱可用",
            lambda: check_sandbox(cfg, docker_client_factory=deps.docker_client_factory),
        ))

    checks.append(_guard("storage", "落盘目录可写", lambda: check_storage(cfg)))
    return Report(checks=checks, notes=notes, config_path=shown, offline=offline)


def _guard(name: str, title: str, fn: Callable[[], CheckResult]) -> CheckResult:
    try:
        return fn()
    except Exception as exc:                        # noqa: BLE001
        return _crashed(name, title, exc)


async def _guard_async(
    name: str, title: str, factory: Callable[[], Any]
) -> CheckResult:
    try:
        return await factory()
    except Exception as exc:                        # noqa: BLE001
        return _crashed(name, title, exc)


# ---------------------------------------------------------------------------
# 渲染
# ---------------------------------------------------------------------------

_TITLE_WIDTH = 16
#: 第 4 组之后插 §3.7 的提示行，位置就是派单说的地方。
_NOTES_AFTER = "feishu_identity"


def render_text(report: Report, redactor: Redactor, out) -> None:
    scrub = redactor.scrub
    total = len(report.checks)
    mode = "--offline（只跑 1/2/7）" if report.offline else "完整（七组）"
    print("Aite 起飞前自检", file=out)
    print(f"  配置：{scrub(report.config_path)}", file=out)
    print(f"  模式：{mode}", file=out)
    print(f"  时间：{datetime.now().astimezone().isoformat(timespec='seconds')}", file=out)
    print(file=out)

    for i, c in enumerate(report.checks, 1):
        label = _STATUS_LABEL[c.status]
        print(f"[{i}/{total}] {label:<4} {_pad(c.title, _TITLE_WIDTH)} {scrub(c.detail)}", file=out)
        if c.fix and c.status != STATUS_OK:
            print(f"           └ 怎么补：{scrub(c.fix)}", file=out)
        if c.name == _NOTES_AFTER and report.notes:
            for note in report.notes:
                print(f"       NOTE {_pad('§3.7 待核实', _TITLE_WIDTH)} {scrub(note.detail)}", file=out)

    counts = {s: sum(1 for c in report.checks if c.status == s) for s in _STATUS_LABEL}
    print(file=out)
    print("-" * 72, file=out)
    print(
        "汇总："
        + " · ".join(f"{_STATUS_LABEL[s]} {counts[s]}" for s in (STATUS_OK, STATUS_WARN, STATUS_FAIL, STATUS_SKIP))
        + f"（共 {total} 项，过了 {report.passed} 项）",
        file=out,
    )
    failed = [c.title for c in report.checks if c.failed]
    if failed:
        print(f"FAIL：{'、'.join(failed)} —— 起飞前把上面的「怎么补」做掉再跑一次。", file=out)
    else:
        print("全部没红，可以起飞。" + ("（--offline 只验了 1/2/7，真机起飞前请全跑一遍）" if report.offline else ""), file=out)


def render_json(report: Report, redactor: Redactor, out) -> None:
    scrub = redactor.scrub
    payload = {
        "ok": report.ok,
        "offline": report.offline,
        "config_path": scrub(report.config_path),
        "passed": report.passed,
        "total": len(report.checks),
        "checks": [
            {
                "name": c.name,
                "title": c.title,
                "status": c.status,
                "detail": scrub(c.detail),
                "fix": scrub(c.fix),
                "extra": redactor.scrub_obj(c.extra),
            }
            for c in report.checks
        ],
        "notes": [
            {"name": n.name, "status": n.status, "detail": scrub(n.detail)} for n in report.notes
        ],
    }
    print(json.dumps(payload, ensure_ascii=False, indent=2), file=out)


# ---------------------------------------------------------------------------
# 入口
# ---------------------------------------------------------------------------

def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="preflight",
        description="Aite 起飞前自检：把外部依赖挨个点名，每项 OK/FAIL 加一句怎么补。",
        epilog="退出码：任一 FAIL → 1，否则 0。任何模式下都不打印密钥取值。",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument(
        "--config",
        default=None,
        help=f"配置文件路径（默认 {CONFIG_PATH}，不存在则退到 {EXAMPLE_CONFIG_PATH}）",
    )
    p.add_argument(
        "--offline",
        action="store_true",
        help="只跑 1 配置 / 2 环境变量 / 7 落盘目录，不碰网络也不碰 docker（CI、没凭证的机器）",
    )
    p.add_argument("--json", action="store_true", help="机器可读输出（每项 name / status / detail）")
    p.add_argument(
        "--chat-id",
        default=None,
        help="测试群的 chat_id；给了就顺带实测一次群历史，回答 §3.7(b) 那条待核实项",
    )
    return p


def _pick_config(explicit: str | None) -> tuple[Path, bool]:
    if explicit:
        return _resolve(explicit), False
    real = _resolve(CONFIG_PATH)
    if real.exists():
        return real, False
    return _resolve(EXAMPLE_CONFIG_PATH), True


def main(
    argv: list[str] | None = None,
    *,
    env: Mapping[str, str] | None = None,
    deps: Deps | None = None,
    out=None,
) -> int:
    args = build_parser().parse_args(argv)
    env = os.environ if env is None else env
    deps = deps or Deps()
    out = out or sys.stdout

    config_path, fell_back = _pick_config(args.config)
    redactor = Redactor()
    report = asyncio.run(
        run_checks(
            config_path=config_path,
            display_path=_rel(config_path),
            fell_back=fell_back,
            env=env,
            offline=args.offline,
            chat_id=args.chat_id,
            redactor=redactor,
            deps=deps,
        )
    )
    (render_json if args.json else render_text)(report, redactor, out)
    return 0 if report.ok else 1


if __name__ == "__main__":
    sys.exit(main())
