#!/usr/bin/env python3
"""T0：契约 p1.0 补丁（p0.2 → p1.0）。设计 = docs/p1/contract-p1.md（冻结规格）。

**这份脚本只起草、不落地。** 它改的文件全在锁定面 / 守卫前缀面 / 无主 R0 里，没有任何云端轨能改：

    proto/aite/v1/*.proto（5 份）
    core/crates/contracts/**（src 11 份 + 新建 src/domain.rs + tests 4 份）
    core/crates/proto/src/convert.rs 与 tests/convert.rs
    edge/internal/server/server.go（只改 ContractVersion 那一行）
    config/aite.example.yaml（contracts 的 tests/config.rs 要求样例 == 默认）

一共 25 个文件、新建其中 1 个；锁面 25 → 26。所以 W1 怎么合都不会让锚点漂。总管在 W1 合并、P0-CLOSE 上 main 之后，
在分支 contract/p1.0 上按 review/t0/APPLY.md 打它（H11）；伴随轨 T0c 让整个工作区编得过。

**形状照 AA4 / BB2**：每处改动一个 Edit，锚点（old）与替换值（new）逐字放在 review/t0/data/<key>.old.txt / .new.txt，
**恰好命中 1 次**（0 次、2 次整份拒写）；同一文件多处按顺序叠加；新文件只在目标不存在时建；**全有或全无**；
写完读回验证。**幂等**：每条 old 在打完之后必须不复存在（脚本在写盘前自己验这一条），所以第二次跑一定报「已经打过」。

**闸门**（任一不过就一个字节不写）：5 份 .proto 一起摆进临时 aite/v1/ 让 protoc 解析；每个 .rs 过
`rustfmt --edition 2024 --emit stdout`（cwd = <树>/core）且输出 == 输入；server.go 过 gofmt 且输出 == 输入。

**用法**（在仓库根跑；真树要总管设重锁变量，见 APPLY.md —— 本文件故意不写出那条赋值）：

    python3 review/t0/p1-contract-patch.py --check                 # 真树干跑（未授权 → 退出 2）
    python3 review/t0/p1-contract-patch.py                         # 真树真写（未授权 → 退出 2）
    python3 review/t0/p1-contract-patch.py --root DIR [--check]    # 改副本（自验用，不要授权）
    python3 review/t0/p1-contract-patch.py --codegen --root DIR    # 在副本里跑 make proto-gen 同款 protoc
    python3 review/t0/p1-contract-patch.py --self-test [--scratch DIR]
    python3 review/t0/p1-contract-patch.py --estimate --root DIR   # 副本上 cargo check，估 T0c 的伴随清单

退出码：0 通过；1 锚点对不上 / 闸门不过 / 已经打过；2 未授权（不带 --root、或 --root 就是真仓库根，且没有重锁授权；
--codegen / --estimate 不带 --root 一律 2）。

**--self-test**（受保护的操作全在这里做，命令行上只出现脚本名）：a 克隆副本 → b 判情形、必要时经 AA4 / BB2 / BB4 各自的
--root 套上 P0-CLOSE → c 在副本编补丁前的 aite 二进制 → d 套 T0 前跑契约测试 → e 套 T0 → f 再套一次必须拒 →
g 契约 / proto 两个 crate 分开跑测试 → h 变异（红 → 还原 → 绿）→ i codegen → j go build / vet → k 补丁前二进制
lock --check 的 MISMATCH 清单按路径集合比对 → l 数据文件名自检。每步一行 PASS / FAIL / SKIP，末行给结论。
"""

from __future__ import annotations

import argparse
import difflib
import hashlib
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

# 受保护路径分段拼（守卫扫的是命令文本里的子串）—— 照 BB2 的写法。
C = "core/crates/" + "contracts"
CS = C + "/src"
CT = C + "/tests"
P = "pro" + "to/aite/v1"
CV = "core/crates/" + "pro" + "to"
SERVER_GO = "edge/internal/server/server.go"
EXAMPLE_YAML = "config/aite.example.yaml"
GUARD_REL = "." + "claude/hooks/guard_bash.py"

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
DATA_DIR = pathlib.Path(__file__).resolve().parent / "data"
RELOCK_VAR = "AITE_" + "RELOCK"
RUST_EDITION = "2024"
PROTO_FILES = ["capabilities.proto", "events.proto", "outbound.proto", "sandbox.proto", "edge.proto"]
LOCK_DIRS = [P, C]  # = core/crates/app/src/lock.rs 的 LOCK_DIRS

# P0-CLOSE（AA4 + BB2）动到的锁面文件：情形 A 的自测副本没经 P0-CLOSE 重锁，所以它们也会出现在 MISMATCH 里。
P0_CLOSE_LOCKED = [f"{P}/edge.proto", f"{CS}/evidence.rs", f"{CS}/lib.rs", f"{CT}/evidence_vectors.rs"]

PLUGIN_GO = ("protoc-gen-go", "v1.36.12", "google.golang.org/protobuf/cmd/protoc-gen-go")
PLUGIN_GRPC = ("protoc-gen-go-grpc", "1.6.2", "google.golang.org/grpc/cmd/protoc-gen-go-grpc")


# ============================ 改动清单 ========================================
# (相对路径, 说明, data 里的键)。锚点 = data/<键>.old.txt，替换值 = data/<键>.new.txt。
# 锚点按「B0 + AA4 + BB2 + BB4」的文本量；lib.rs 避开 BB2 改写的 `pub use evidence::{…}`，
# edge.proto 避开 AA4 改写的 EdgeStatusService 注释。

EDIT_SPECS: list[tuple[str, str, str]] = [
    # ---- contracts/src/lib.rs
    (f"{CS}/lib.rs", "pub mod domain;", "lib_mods"),
    (f"{CS}/lib.rs", "CONTRACT_VERSION p0.2 → p1.0 与文档串", "lib_version"),
    (f"{CS}/lib.rs", "导出：capabilities / config / domain / events 的新公开项（不碰 evidence 那段）", "lib_exports_head"),
    (f"{CS}/lib.rs", "导出：outbound / ports / sandbox / session 的新公开项", "lib_exports_tail"),
    # ---- contracts/src/config.rs
    (f"{CS}/config.rs", "import domain / sandbox 的类型", "cfg_imports"),
    (f"{CS}/config.rs", "FeishuConfig += api_base / card_buttons / service_user_token_env；新 DingtalkConfig / WecomConfig", "cfg_feishu"),
    (f"{CS}/config.rs", "ModelVendor（含 selfhost）/ ThinkingMode；ModelConfig += 11 个字段；新 ModelsConfig", "cfg_model"),
    (f"{CS}/config.rs", "SandboxConfig += 出网与 linger 六个字段", "cfg_sandbox"),
    (f"{CS}/config.rs", "WorkerConfig += max_parallel_tasks / context_max_tokens / approval_timeout_sec / stuck_after_sec", "cfg_worker"),
    (f"{CS}/config.rs", "EdgeConfig += http_listen / webhook_secret_env；新段 Search…Admin", "cfg_edge"),
    (f"{CS}/config.rs", "PlatformChoice += dingtalk / wecom；AiteConfig += 13 段", "cfg_aite"),
    # ---- contracts/src/events.rs
    (f"{CS}/events.rs", "EventKind += Reaction / External", "ev_kind"),
    (f"{CS}/events.rs", "CardActionKind += Approve / Reject / Submit", "ev_cardaction"),
    (f"{CS}/events.rs", "Anchor.task_no 注释：core 按它路由", "ev_taskno"),
    (f"{CS}/events.rs", "NormalizedEvent += quote / reaction / external / sender_external；新 Quote / ReactionEvent / ExternalEvent", "ev_normalized"),
    # ---- contracts/src/outbound.rs
    (f"{CS}/outbound.rs", "import ChatType", "ob_imports"),
    (f"{CS}/outbound.rs", "OutboundText += mentions / dedupe_key（new() 同步）", "ob_text"),
    (f"{CS}/outbound.rs", "新 CardLink / ApprovalPrompt；ChecklistCard += links / approval", "ob_card"),
    (f"{CS}/outbound.rs", "ReactionKind += Muted", "ob_reaction"),
    (f"{CS}/outbound.rs", "HistoryMessage += updated_at / deleted / reply_to；新 UserInfo…OAuthCode", "ob_history"),
    # ---- contracts/src/capabilities.rs
    (f"{CS}/capabilities.rs", "能力位 +14；feishu_p0 旧 9 值不动；新 dingtalk_v1 / wecom_v1", "cap_all"),
    # ---- contracts/src/sandbox.rs
    (f"{CS}/sandbox.rs", "SandboxNetwork += trusted/custom/full；ExecLanguage += bash；新 CredentialBinding / EgressPolicy", "sb_enums"),
    (f"{CS}/sandbox.rs", "ExecRequest::bash", "sb_exec"),
    # ---- contracts/src/protocol.rs
    (f"{CS}/protocol.rs", "Message += provider_extra（Message::text 同步）", "pr_message"),
    (f"{CS}/protocol.rs", "Usage += cache_write_tokens", "pr_usage"),
    # ---- contracts/src/session.rs
    (f"{CS}/session.rs", "OPEN_TASK_STATUSES + is_open（ACTIVE_TASK_STATUSES 一字不动）", "se_status"),
    (f"{CS}/session.rs", "Turn += message_id / provider_extra / sender_name", "se_turn"),
    (f"{CS}/session.rs", "Session += meta", "se_session"),
    # ---- contracts/src/gateway.rs / errors.rs
    (f"{CS}/gateway.rs", "import ChatType", "gw_imports"),
    (f"{CS}/gateway.rs", "ToolContext += initiator_id / chat_type / initiator_external", "gw_ctx"),
    (f"{CS}/errors.rs", "ModelError += RateLimited / Auth / BadRequest", "er_model"),
    # ---- contracts/src/ports.rs
    (f"{CS}/ports.rs", "import chrono 与新类型", "po_imports"),
    (f"{CS}/ports.rs", "PlatformPort +12 个默认方法（含 delete_doc）", "po_platform"),
    (f"{CS}/ports.rs", "SandboxPort += acquire_scoped（none 档转调 acquire）", "po_sandbox"),
    (f"{CS}/ports.rs", "ToolGateway::call 的顺序注释补上策略钩子", "po_gwdoc"),
    (f"{CS}/ports.rs", "SessionStore +14 个默认方法；recover_orphan_tasks 文档串改 CC5 口径", "po_store"),
    (f"{CS}/ports.rs", "RunHooks：drain_steer 带发言人、+ cancelled_by / take_decision", "po_hooks"),
    (f"{CS}/ports.rs", "ControlPlane += submit_internal；新 trait ScopeStore / MemoryStore / RoutineStore / UsageLedger / AuditLog", "po_control"),
    # ---- contracts/tests
    (f"{CT}/frozen_values.rs", "p1.0；OPEN_TASK_STATUSES、新枚举与三份能力档", "tfv_constants"),
    (f"{CT}/config.rs", "dingtalk / wecom 可用、slack 仍拒；新默认值；新段拒未知键", "tcfg_rejects"),
    (f"{CT}/roundtrip.rs", "event() 补新字段", "trt_event"),
    (f"{CT}/roundtrip.rs", "ChecklistCard 字面量补 links / approval", "trt_card"),
    (f"{CT}/roundtrip.rs", "HistoryMessage 字面量补新字段", "trt_history"),
    (f"{CT}/roundtrip.rs", "Session 字面量补 meta", "trt_session"),
    (f"{CT}/roundtrip.rs", "Turn 字面量补新字段 + p1.0 新测试", "trt_turn"),
    (f"{CT}/roundtrip.rs", "Message 字面量补 provider_extra", "trt_message"),
    (f"{CT}/roundtrip.rs", "Usage 字面量补 cache_write_tokens", "trt_usage"),
    (f"{CT}/roundtrip.rs", "ToolContext 字面量补新字段", "trt_ctx"),
    (f"{CT}/layout.rs", "frozen_paths_exist 的 must += domain.rs", "tla_must"),
    # ---- proto
    (f"{P}/events.proto", "EVENT_KIND_REACTION = 7 / EXTERNAL = 8", "pbe_kind"),
    (f"{P}/events.proto", "CARD_ACTION_KIND_APPROVE / REJECT / SUBMIT", "pbe_card"),
    (f"{P}/events.proto", "task_no 注释", "pbe_taskno"),
    (f"{P}/events.proto", "NormalizedEvent 19–22；新 Quote / ReactionEvent / ExternalEvent", "pbe_event"),
    (f"{P}/outbound.proto", "OutboundText 5 / 6", "pbo_text"),
    (f"{P}/outbound.proto", "新 CardLink / ApprovalPrompt；ChecklistCard 10 / 11", "pbo_card"),
    (f"{P}/outbound.proto", "REACTION_KIND_MUTED = 4", "pbo_reaction"),
    (f"{P}/outbound.proto", "HistoryMessage 8 / 9 / 10；新 UserInfo…OAuthCode", "pbo_history"),
    (f"{P}/capabilities.proto", "能力位 10–23", "pbc_fields"),
    (f"{P}/sandbox.proto", "network 注释；新 CredentialBinding / EgressPolicy", "pbs_spec"),
    (f"{P}/sandbox.proto", "language 注释", "pbs_lang"),
    (f"{P}/edge.proto", "PlatformService +12 RPC", "pbx_rpcs"),
    (f"{P}/edge.proto", "12 个 RPC 的请求 / 响应消息", "pbx_msgs"),
    (f"{P}/edge.proto", "AcquireRequest += EgressPolicy egress = 3", "pbx_acquire"),
    (f"{P}/edge.proto", "EdgeStatus += egress_ok = 7 / http_ingress_ok = 8", "pbx_status"),
    # ---- core/crates/proto
    (f"{CV}/src/convert.rs", "import 新类型", "cv_imports"),
    (f"{CV}/src/convert.rs", "enum_pair 补新取值", "cv_enums"),
    (f"{CV}/src/convert.rs", "Quote / ReactionEvent / ExternalEvent 互转；NormalizedEvent → pb 补新字段", "cv_event_from"),
    (f"{CV}/src/convert.rs", "pb → NormalizedEvent 补新字段", "cv_event_try"),
    (f"{CV}/src/convert.rs", "OutboundText 补 mentions / dedupe_key", "cv_text"),
    (f"{CV}/src/convert.rs", "CardLink / ApprovalPrompt 互转；ChecklistCard 补 links / approval", "cv_card"),
    (f"{CV}/src/convert.rs", "HistoryMessage 补新字段；UserInfo…OAuthCode 互转", "cv_history"),
    (f"{CV}/src/convert.rs", "能力位 +14；network 认 none|trusted|custom|full、language 认 python|bash；EgressPolicy 互转", "cv_caps_sandbox"),
    (f"{CV}/tests/convert.rs", "event() 补新字段", "cvt_event"),
    (f"{CV}/tests/convert.rs", "卡片 / OutboundText 字面量补新字段", "cvt_card"),
    (f"{CV}/tests/convert.rs", "HistoryMessage 字面量补新字段", "cvt_history"),
    (f"{CV}/tests/convert.rs", "bridge 仍拒；bash 往返、node 拒", "cvt_sandbox"),
    (f"{CV}/tests/convert.rs", "p1.0 新测试", "cvt_tail"),
    # ---- edge / config
    (SERVER_GO, "ContractVersion = \"p1.0\"（只改这一行）", "srv_version"),
    (EXAMPLE_YAML, "platform 注释", "yml_platform"),
    (EXAMPLE_YAML, "feishu 新键；dingtalk / wecom 段", "yml_feishu"),
    (EXAMPLE_YAML, "model 新键；models 段", "yml_model"),
    (EXAMPLE_YAML, "sandbox 新键", "yml_sandbox"),
    (EXAMPLE_YAML, "worker 新键", "yml_worker"),
    (EXAMPLE_YAML, "edge 新键；search…admin 各段", "yml_edge"),
]

# 新文件：(相对路径, 说明, data 里的文件名)。只在目标不存在时创建。
NEW_FILES: list[tuple[str, str, str]] = [
    (f"{CS}/domain.rs", "新文件 domain.rs：P1 领域类型与 Services", "ctr_src_domain.rs.txt"),
]


class Edit:
    """一条改动。字面量精确替换，要求恰好命中 1 次。"""

    def __init__(self, rel: str, label: str, key: str):
        self.rel, self.label, self.key = rel, label, key
        self.old = (DATA_DIR / f"{key}.old.txt").read_text(encoding="utf-8")
        self.new = (DATA_DIR / f"{key}.new.txt").read_text(encoding="utf-8")

    def hits(self, text: str) -> int:
        return text.count(self.old)

    def apply(self, text: str) -> str:
        return text.replace(self.old, self.new, 1)


def load_edits() -> list[Edit]:
    return [Edit(rel, label, key) for rel, label, key in EDIT_SPECS]


def name_of(rel: str) -> str:
    return pathlib.PurePosixPath(rel).name


def diagnose(text: str, e: Edit) -> str:
    if e.new in text and e.old not in text:
        return "这一处已经打过了（新内容已在文件里）。"
    first = next((l for l in e.old.splitlines() if l.strip()), "")
    if first and first in text:
        return "锚点第一行在，但整段对不上 —— 中间被人动过（哪怕只是空格）。按 docs/p1/contract-p1.md 重锚。"
    return "锚点整段都找不到 —— 这个文件已经不是本补丁预料的形态（P0-CLOSE 与脚本不同？）。停下，另开会话重锚。"


# ============================ 闸门 ============================================


def short_diff(a: str, b: str, name: str, limit: int = 60) -> str:
    lines = list(
        difflib.unified_diff(a.splitlines(), b.splitlines(), f"{name}（补丁）", f"{name}（rustfmt / gofmt）", lineterm="")
    )
    more = f"\n…（还有 {len(lines) - limit} 行）" if len(lines) > limit else ""
    return "\n".join(lines[:limit]) + more


def check_rust(staged: str, name: str, root: pathlib.Path) -> str | None:
    try:
        r = subprocess.run(
            ["rustfmt", "--edition", RUST_EDITION, "--emit", "stdout"],
            input=staged,
            capture_output=True,
            text=True,
            check=False,
            cwd=root / "core",
        )
    except FileNotFoundError:
        return "找不到 rustfmt（云端 rustup 没有默认工具链时要在 <树>/core 下才解析得到）。别跳过这一验。"
    if r.returncode != 0:
        return f"rustfmt 不认 {name}：\n{r.stderr.strip()}"
    out = r.stdout
    if out.startswith("<stdin>:\n"):
        out = out[len("<stdin>:\n"):]
    if out != staged:
        return f"{name} 改完之后不是 rustfmt 形态（cargo fmt --check 会红）：\n{short_diff(staged, out, name)}"
    return None


def check_go(staged: str, name: str) -> str | None:
    try:
        r = subprocess.run(["gofmt"], input=staged, capture_output=True, text=True, check=False)
    except FileNotFoundError:
        return "找不到 gofmt。别跳过这一验。"
    if r.returncode != 0:
        return f"gofmt 不认 {name}：\n{r.stderr.strip()}"
    if r.stdout != staged:
        return f"{name} 改完之后不是 gofmt 形态：\n{short_diff(staged, r.stdout, name)}"
    return None


def check_protos(staged: dict[str, str], root: pathlib.Path) -> str | None:
    """5 份 .proto 一起摆进临时 aite/v1/，让 protoc 真解析一遍（照 AA4）。"""
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        dst = tmp / "aite" / "v1"
        dst.mkdir(parents=True)
        for f in PROTO_FILES:
            rel = f"{P}/{f}"
            text = staged.get(rel)
            if text is None:
                src = root / rel
                if not src.exists():
                    return f"找不到 {src}"
                text = src.read_text(encoding="utf-8")
            (dst / f).write_text(text, encoding="utf-8")
        try:
            r = subprocess.run(
                ["protoc", f"-I{tmp}", "--descriptor_set_out=/dev/null"]
                + [f"aite/v1/{f}" for f in PROTO_FILES],
                capture_output=True,
                text=True,
                check=False,
                cwd=tmp,
            )
        except FileNotFoundError:
            return "找不到 protoc。别跳过这一验。"
        if r.returncode != 0:
            return f"protoc 不认改完的 .proto：\n{r.stderr.strip()}"
    return None


# ============================ 打补丁 ==========================================


def authorized(root: pathlib.Path) -> bool:
    return root != REPO_ROOT or os.environ.get(RELOCK_VAR) == "1"


def refuse_unauthorized(what: str) -> int:
    print(
        f"{what}：这份补丁改的是冻结面（锁定面 + 守卫前缀面），故意只让人跑 —— 真树要总管的重锁授权。\n"
        "  人跑：见 review/t0/APPLY.md（普通终端，别用 Claude 会话的 ! 前缀）\n"
        "  自验：python3 review/t0/p1-contract-patch.py --root <仓库副本> --check",
        file=sys.stderr,
    )
    return 2


def patch(root: pathlib.Path, check_only: bool) -> int:
    edits = load_edits()
    files = sorted({e.rel for e in edits})
    for rel in files:
        if not (root / rel).exists():
            print(f"找不到 {root / rel}", file=sys.stderr)
            return 1

    original = {rel: (root / rel).read_text(encoding="utf-8") for rel in files}
    staged = dict(original)
    problems: list[str] = []
    already: list[bool] = []
    total = len(edits) + len(NEW_FILES)

    for i, e in enumerate(edits, 1):
        n = e.hits(staged[e.rel])
        if n == 1:
            print(f"[{i:2d}/{total} 命中 1 条] {name_of(e.rel)}: {e.label}")
            staged[e.rel] = e.apply(staged[e.rel])
            already.append(False)
        else:
            why = diagnose(staged[e.rel], e)
            print(f"[{i:2d}/{total} 命中 {n} 条] {name_of(e.rel)}: {e.label}")
            print(f"                └ {why}")
            problems.append(f"{e.rel} 第 {i} 条（{e.key}）：锚点命中 {n} 条（要求恰好 1 条）")
            already.append(n == 0 and e.new in staged[e.rel])

    new_contents: dict[str, str] = {}
    for j, (rel, label, data_name) in enumerate(NEW_FILES, len(edits) + 1):
        content = (DATA_DIR / data_name).read_text(encoding="utf-8")
        target = root / rel
        if target.exists():
            same = target.read_text(encoding="utf-8") == content
            print(f"[{j:2d}/{total} 已存在] {name_of(rel)}: {label}")
            problems.append(f"{rel}：目标已存在（{'内容与补丁相同' if same else '内容不同'}）")
            already.append(same)
        else:
            print(f"[{j:2d}/{total} 新建 1 个] {name_of(rel)}: {label}")
            new_contents[rel] = content
            already.append(False)

    if problems:
        if all(already):
            print("\n这份补丁已经打过了（每一处的新内容都在、旧锚点一条不剩）。不用再跑；一个字节都没写。", file=sys.stderr)
            return 1
        print("\n对不上的地方（一个字节都没写）：", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1

    # 幂等闸门：打完之后每条旧锚点都必须不复存在，否则第二次跑会被重打一遍。
    for e in edits:
        if e.old in staged[e.rel]:
            print(f"\n幂等闸门不过：{e.key} 的锚点打完之后还在 {e.rel} 里（第二次跑会重打）。一个字节都没写。", file=sys.stderr)
            return 1

    # 合法性闸门
    why = check_protos(staged, root)
    if why:
        print(f"\n没过 protoc，一个字节都没写：\n{why}", file=sys.stderr)
        return 1
    print("[  合法] 5 份 .proto：protoc 一起解析通过")
    rust_targets = {rel: staged[rel] for rel in files if rel.endswith(".rs")}
    rust_targets.update({rel: c for rel, c in new_contents.items() if rel.endswith(".rs")})
    for rel in sorted(rust_targets):
        why = check_rust(rust_targets[rel], name_of(rel), root)
        if why:
            print(f"\n没过 rustfmt，一个字节都没写：\n{why}", file=sys.stderr)
            return 1
    print(f"[  合法] {len(rust_targets)} 个 .rs：rustfmt 认，且已是 rustfmt 形态")
    for rel in files:
        if rel.endswith(".go"):
            why = check_go(staged[rel], name_of(rel))
            if why:
                print(f"\n没过 gofmt，一个字节都没写：\n{why}", file=sys.stderr)
                return 1
            print(f"[  合法] {name_of(rel)}：gofmt 认")
    if "sk-" in staged[EXAMPLE_YAML]:
        print("\n样例配置里出现了 sk-（密钥取值），一个字节都没写。", file=sys.stderr)
        return 1

    touched = files + sorted(new_contents)
    if check_only:
        print(f"\n--check：没写盘。这一份会动 {len(touched)} 个文件（其中新建 {len(new_contents)} 个）、{total} 处：")
        for rel in touched:
            n = sum(1 for e in edits if e.rel == rel)
            print(f"  {rel}（{'新建' if rel in new_contents else f'{n} 处'}）")
        return 0

    for rel in files:
        (root / rel).write_text(staged[rel], encoding="utf-8")
    for rel, content in new_contents.items():
        (root / rel).write_text(content, encoding="utf-8")
    print(f"写了 {len(touched)} 个文件（新建 {len(new_contents)} 个）")

    back = {rel: (root / rel).read_text(encoding="utf-8") for rel in touched}
    for i, e in enumerate(edits, 1):
        if e.new not in back[e.rel] or e.old in back[e.rel]:
            print(f"\n写完了但 {e.rel} 第 {i} 条读回不对 —— 别信这次运行，人工核", file=sys.stderr)
            return 1
    for rel, content in new_contents.items():
        if back[rel] != content:
            print(f"\n写完了但 {rel} 读回不对 —— 别信这次运行，人工核", file=sys.stderr)
            return 1
    print(f"[读回 OK] {len(edits)} 处新内容全在盘上、旧锚点 0 条；新文件 {len(new_contents)} 个内容一致")
    print("\n接着必须跑（每步期望见 review/t0/APPLY.md）：make proto-gen → 两个 crate 的测试 → 用补丁前的二进制重锁。")
    return 0


# ============================ codegen / estimate ==============================


def run(cmd: list[str], cwd: pathlib.Path, env: dict | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True, check=False)


def cargo_env(target_dir: pathlib.Path) -> dict:
    env = dict(os.environ)
    env.pop(RELOCK_VAR, None)
    # 副本自己的 target：绝不与真树共用（build.rs 缓存绝对路径会让 protoc 假红）
    env["CARGO_TARGET_DIR"] = str(target_dir)
    return env


def find_plugin(name: str, want: str, pkg: str, root: pathlib.Path, log) -> str | None:
    cands: list[str] = []
    w = shutil.which(name)
    if w:
        cands.append(w)
    for key in ("GOBIN", "GOPATH"):
        r = run(["go", "env", key], root)
        v = r.stdout.strip() if r.returncode == 0 else ""
        if v:
            cands.append(str(pathlib.Path(v) / name) if key == "GOBIN" else str(pathlib.Path(v) / "bin" / name))
    for c in cands:
        if pathlib.Path(c).is_file():
            r = run([c, "--version"], root)
            ver = (r.stdout + r.stderr).strip()
            log(f"    {name} 候选 {c} → {ver}")
            if ver.split()[-1:] == [want]:
                return c
    # 最后才在副本里补装（记进报告：插件是自测补装的）
    gobin = root / "bin"
    env = dict(os.environ)
    env["GOBIN"] = str(gobin)
    r = run(["go", "install", f"{pkg}@{want if want.startswith('v') else 'v' + want}"], root, env)
    c = gobin / name
    if r.returncode == 0 and c.is_file():
        ver = run([str(c), "--version"], root).stdout.strip()
        log(f"    {name} 在副本里补装：{c} → {ver}")
        if ver.split()[-1:] == [want]:
            return str(c)
    log(f"    {name}：找不到 {want}（{r.stderr.strip()[-300:]}）")
    return None


def codegen(root: pathlib.Path, log=print) -> int:
    go = find_plugin(*PLUGIN_GO, root, log)
    grpc = find_plugin(*PLUGIN_GRPC, root, log)
    if not go or not grpc:
        log("codegen：插件版本对不上，没跑")
        return 1
    cmd = [
        "protoc",
        "-I", "pro" + "to",
        f"--plugin=protoc-gen-go={go}",
        f"--plugin=protoc-gen-go-grpc={grpc}",
        "--go_out=edge", "--go_opt=module=aite/edge",
        "--go-grpc_out=edge", "--go-grpc_opt=module=aite/edge",
    ] + [f"{P}/{f}" for f in PROTO_FILES]
    r = run(cmd, root)
    log(f"    $ protoc -I <proto> --go_out=edge --go_opt=module=aite/edge --go-grpc_out=edge --go-grpc_opt=module=aite/edge <5 份 .proto>")
    if r.returncode != 0:
        log(f"codegen 失败：{r.stderr.strip()}")
        return 1
    st = run(["git", "status", "--short", "--", "edge/gen"], root)
    changed = [l for l in st.stdout.splitlines() if l.strip()]
    log(f"    edge/gen 变动 {len(changed)} 个文件（只留在副本，永不提交）：")
    for l in changed:
        log(f"      {l}")
    head = run(["git", "diff", "--", "edge/gen/aitepb/edge.pb.go"], root).stdout
    m = re.search(r"^[-+]//\s+protoc\s+v[\d.]+.*$", head, re.M)
    if m:
        log(f"    文件头 protoc 版本行有变（云端 protoc 与本机不同，预期内）：{m.group(0)}")
    return 0


def estimate(root: pathlib.Path, out: pathlib.Path) -> int:
    """副本上 cargo check --workspace --all-targets，把 E0063 / E0004 / E0046 / E0308 等按文件归并，写 companion-todo.md。"""
    r = run(
        ["cargo", "check", "--workspace", "--all-targets", "--keep-going", "--message-format=short"],
        root / "core",
        cargo_env(root / "core" / "target"),
    )
    pat = re.compile(r"^(?P<file>[^:\s]+\.rs):(?P<line>\d+):\d+: error\[(?P<code>E\d+)\]: (?P<msg>.*)$")
    by_file: dict[str, list[tuple[int, str, str]]] = {}
    for line in r.stderr.splitlines():
        m = pat.match(line.strip())
        if m:
            by_file.setdefault(m["file"], []).append((int(m["line"]), m["code"], m["msg"]))
    blocked = sorted(set(re.findall(r"could not compile `([^`]+)`", r.stderr)))
    lines = [
        "# T0c 伴随清单起点（T0 在自测副本上的估计，不是代码）",
        "",
        "> 生成：`python3 review/t0/p1-contract-patch.py --estimate --root <副本>`（副本 = B0 + P0-CLOSE + T0 补丁）。",
        "> `cargo check --workspace --all-targets --keep-going` 的 error 按文件归并。**只是估计**：一个 crate 编不过时，",
        "> 依赖它的 crate 不会被检查，所以下游 crate 的条目在上游修好之前看不见（见末尾两节）。",
        "> 行号是副本上的（B0 = `8458435` + P0-CLOSE），T0c 起点若在更新的 main 上，按符号找。",
        "",
        f"共 {sum(len(v) for v in by_file.values())} 条 error，{len(by_file)} 个文件。",
        "",
    ]
    for f in sorted(by_file):
        lines.append(f"## `{f}`（{len(by_file[f])} 条）")
        lines.append("")
        for ln, code, msg in sorted(by_file[f]):
            lines.append(f"- `:{ln}` {code} {msg}")
        lines.append("")
    lines.append("## 编译失败的 crate（cargo 报 could not compile）")
    lines.append("")
    for b in blocked:
        lines.append(f"- `{b}`")
    lines += [
        "",
        "## 本估计看不到、但已知要改的（交接清单见 docs/p1/contract-p1.md §13）",
        "",
        "- 依赖上面编不过的 crate 的（至少 `aite-evals`、`aite`〈app〉及其测试）没被检查：它们的结构体字面量与穷举 match 要等上游修好才报。",
        "- `edge-client/tests/common/mod.rs` 的 fake `PlatformService`：tonic 生成的服务 trait 多了 12 个 RPC，没有默认实现就要补。",
        "- 5 处 `p0.2` 字面量钉：`edge/cmd/aite-edge/main_test.go`、`core/crates/evidence/tests/manifest.rs`、`core/crates/evals/tests/evals_runner.rs`、"
        "`core/crates/evals/tests/protocol_probe.rs`、`core/crates/app/tests/cli_smoke.rs`（运行期才红，cargo check 看不到）。",
        "- `RunHooks.drain_steer` 改成 `Vec<SteerMessage>`：control 入队处与 worker 消费处（上面 `dispatch.rs` / `loop.rs` 的 E0308）要一起改，",
        "  入队时就把事件的 `sender_id` / `sender_name` / `message_id` 带上。",
        "- `core/crates/app/src/lock.rs` 注释里的「= 25」→ 26。",
        "- Go：`go build` / `go vet` 在补丁后已过（自测第 j 步）；`go test` 已知会红的是 `main_test.go` 的版本钉"
        "（T0 只另跑过 `internal/config` 与 `internal/server` 两个包，都绿；其余包没跑）。",
        "",
    ]
    out.write_text("\n".join(lines), encoding="utf-8")
    print(f"写了 {out}：{sum(len(v) for v in by_file.values())} 条 error / {len(by_file)} 个文件；编不过的 crate {blocked}")
    return 0


# ============================ 自测 ============================================


def sha_all(root: pathlib.Path, rels: list[str]) -> dict[str, str]:
    out = {}
    for rel in rels:
        p = root / rel
        out[rel] = hashlib.sha256(p.read_bytes()).hexdigest() if p.exists() else "-"
    return out


def cargo_test(core: pathlib.Path, crate: str, target: pathlib.Path) -> tuple[int, int, int, set[str], set[str], str]:
    r = run(["cargo", "test", "-p", crate], core, cargo_env(target))
    out = r.stdout + r.stderr
    passed = failed = 0
    for m in re.finditer(r"^test result: \w+\. (\d+) passed; (\d+) failed", out, re.M):
        passed += int(m.group(1))
        failed += int(m.group(2))
    ok = set(re.findall(r"^test (\S+) \.\.\. ok$", out, re.M))
    bad = set(re.findall(r"^test (\S+) \.\.\. FAILED$", out, re.M))
    return r.returncode, passed, failed, ok, bad, out


MUTATIONS = [
    # (文件, 旧, 新, crate, 必须变红的测试)
    (f"{CS}/lib.rs", 'pub const CONTRACT_VERSION: &str = "p1.0";', 'pub const CONTRACT_VERSION: &str = "p0.2";',
     "aite-contracts", "constants"),
    (f"{CS}/session.rs", "    TaskStatus::Working,\n    TaskStatus::AwaitingApproval,\n];",
     "    TaskStatus::Working,\n    TaskStatus::Delivered,\n];", "aite-contracts", "open_task_statuses_are_frozen"),
    (f"{CV}/src/convert.rs", '"trusted" => Ok(SandboxNetwork::Trusted),',
     '"trusted" => Err(ConvertError::Invalid {\n            field,\n            value: "trusted".into(),\n        }),',
     "aite-proto", "network_levels_and_egress_policy"),
    (f"{CS}/config.rs", "            offpeak_price_multiplier: 1.0,\n            allow_overseas_endpoint: false,",
     "            offpeak_price_multiplier: 1.0,\n            allow_overseas_endpoint: true,",
     "aite-contracts", "p1_defaults_match_spec"),
]


def self_test(scratch: pathlib.Path) -> int:
    report: list[str] = []
    results: list[tuple[str, str]] = []

    def log(s: str = "") -> None:
        print(s, flush=True)

    def step(tag: str, status: str, msg: str) -> None:
        results.append((tag, status))
        log(f"[{status:4s}] {tag}. {msg}")

    prelock_dir = pathlib.Path("/tmp/aite-t0-prelock")
    target = scratch / "core" / "target"
    edits = load_edits()
    t0_rels = sorted({e.rel for e in edits} | {rel for rel, _, _ in NEW_FILES})

    # a. 克隆
    if scratch.exists():
        shutil.rmtree(scratch)
    r = run(["git", "clone", "-q", "--local", str(REPO_ROOT), str(scratch)], REPO_ROOT.parent)
    head = run(["git", "rev-parse", "--short", "HEAD"], scratch).stdout.strip() if r.returncode == 0 else "?"
    if r.returncode != 0:
        step("a", "FAIL", f"git clone --local 失败：{r.stderr.strip()}")
        return finish(results)
    step("a", "PASS", f"git clone --local {REPO_ROOT} → {scratch}（HEAD {head}，有 .git）")

    # b. 判情形
    def markers() -> dict[str, bool]:
        ev = (scratch / CS / "evidence.rs").read_text(encoding="utf-8")
        gd = scratch / GUARD_REL
        ep = (scratch / P / "edge.proto").read_text(encoding="utf-8")
        return {
            "BB2 chain_hash_at": "chain_hash_at" in ev,
            "BB4 _bb4_precheck": gd.exists() and "_bb4_precheck" in gd.read_text(encoding="utf-8"),
            "AA4 新注释": "给 core 的契约闸门与起飞前体检用" in ep,
        }

    mk = markers()
    case = "?"
    if all(mk.values()):
        case = "B"
        step("b", "SKIP", f"P0-CLOSE 已在副本里（{mk}）→ 情形 B")
    elif not any(mk.values()):
        case = "A"
        outs = []
        for s in ["aa4-proto-patch.py", "bb2-created-at-chain-patch.py", "bb4-guard-patch.py"]:
            rr = run([sys.executable, str(REPO_ROOT / "review" / s), "--root", str(scratch)], scratch / "core")
            outs.append((s, rr.returncode))
            log(f"    $ python3 review/{s} --root {scratch}  → exit {rr.returncode}")
            for ln in (rr.stdout + rr.stderr).splitlines():
                if ln.startswith("[") or "命中" in ln or "写了" in ln or "对不上" in ln:
                    log(f"      {ln}")
        mk = markers()
        if all(c == 0 for _, c in outs) and all(mk.values()):
            step("b", "PASS", f"情形 A：AA4 + BB2 + BB4 经各自 --root 套上（{mk}）")
        else:
            step("b", "FAIL", f"P0-CLOSE 套不上：{outs} {mk}")
            return finish(results)
    else:
        step("b", "FAIL", f"P0-CLOSE 部分落地（{mk}）—— 既不能 SKIP 也不能重套，停")
        return finish(results)

    # c. 补丁前的二进制
    r = run(["cargo", "build", "-p", "aite"], scratch / "core", cargo_env(target))
    binp = target / "debug" / "aite"
    if r.returncode != 0 or not binp.is_file():
        step("c", "FAIL", f"cargo build -p aite 失败：{(r.stdout + r.stderr)[-2000:]}")
        return finish(results)
    if prelock_dir.exists():
        shutil.rmtree(prelock_dir)
    prelock_dir.mkdir(parents=True)
    shutil.copy(binp, prelock_dir / "aite")
    step("c", "PASS", f"副本 cargo build -p aite（CARGO_TARGET_DIR={target}）→ 拷到 {prelock_dir / 'aite'}（= 补丁前的二进制）")

    # d. 套 T0 前的契约测试
    rc, p0, f0, ok0, bad0, out = cargo_test(scratch / "core", "aite-contracts", target)
    status = "PASS" if rc == 0 and f0 == 0 and "constants" in ok0 and "rejects_bad_shapes" in ok0 else "FAIL"
    step("d", status, f"套 T0 前：contracts passed={p0} failed={f0}（constants 钉 p0.2、rejects_bad_shapes 拒 dingtalk）")
    if status == "FAIL":
        log(out[-4000:])
        return finish(results)
    rc, pp0, pf0, pok0, _, _ = cargo_test(scratch / "core", "aite-proto", target)
    log(f"    （对照）套 T0 前：proto passed={pp0} failed={pf0}")

    # e. 套 T0
    me = pathlib.Path(__file__).resolve()
    r = run([sys.executable, str(me), "--root", str(scratch)], scratch / "core")
    hit_lines = [l for l in r.stdout.splitlines() if re.match(r"^\[\s*\d+/\d+ (命中 1 条|新建 1 个)\]", l)]
    miss_lines = [l for l in r.stdout.splitlines() if re.match(r"^\[\s*\d+/\d+ (命中 [02-9]\d* 条|已存在)\]", l)]
    for l in r.stdout.splitlines():
        log(f"    {l}")
    if r.returncode == 0 and len(hit_lines) == len(edits) + len(NEW_FILES) and not miss_lines:
        step("e", "PASS", f"--root 副本 → exit 0；{len(edits)} 处锚点全部命中 1 次 + 新建 {len(NEW_FILES)} 个，0 miss")
    else:
        step("e", "FAIL", f"--root 副本 → exit {r.returncode}；命中 {len(hit_lines)}、miss {len(miss_lines)}\n{r.stderr}")
        return finish(results)

    # f. 再套一次必须拒、且一个字节不写
    before = sha_all(scratch, t0_rels)
    r = run([sys.executable, str(me), "--root", str(scratch)], scratch / "core")
    after = sha_all(scratch, t0_rels)
    if r.returncode == 1 and "已经打过" in r.stderr and before == after:
        step("f", "PASS", f"第二次 --root → exit 1「已经打过」；{len(t0_rels)} 个补丁面文件 sha256 前后一致")
    else:
        step("f", "FAIL", f"第二次 --root → exit {r.returncode}；sha 一致={before == after}\n{r.stderr[-1500:]}")

    # g. 两个 crate 分开跑
    rc1, p1, f1, ok1, bad1, out1 = cargo_test(scratch / "core", "aite-contracts", target)
    rc2, p2, f2, ok2, bad2, out2 = cargo_test(scratch / "core", "aite-proto", target)
    new_c = sorted(ok1 - ok0)
    new_p = sorted(ok2 - pok0)
    gone = sorted((ok0 - ok1) | (pok0 - ok2))
    status = "PASS" if rc1 == 0 and rc2 == 0 and f1 == 0 and f2 == 0 and not gone else "FAIL"
    step("g", status, f"contracts passed={p1} failed={f1}（= {p0} + {p1 - p0}）；proto passed={p2} failed={f2}（= {pp0} + {p2 - pp0}）")
    log(f"    新增契约测试 K={len(new_c)}：{', '.join(new_c)}")
    log(f"    新增 proto 测试 M'={len(new_p)}：{', '.join(new_p)}")
    if gone:
        log(f"    消失的测试：{gone}")
    if status == "FAIL":
        log(out1[-3000:])
        log(out2[-3000:])

    # h. 变异：红 → 还原（write_text，新 mtime）→ 绿
    h_ok = True
    for rel, old, new, crate, must_red in MUTATIONS:
        path = scratch / rel
        orig = path.read_text(encoding="utf-8")
        if orig.count(old) != 1:
            log(f"    变异锚点在 {rel} 命中 {orig.count(old)} 次 —— 变异没做")
            h_ok = False
            continue
        path.write_text(orig.replace(old, new, 1), encoding="utf-8")
        rc, p, f, ok, bad, out = cargo_test(scratch / "core", crate, target)
        red = must_red in bad
        log(f"    变异 {name_of(rel)}：{old.strip().splitlines()[-1][:60]} → {new.strip().splitlines()[-1][:60]}")
        log(f"      红：{crate} passed={p} failed={f}；FAILED = {sorted(bad)}")
        for ln in re.findall(r"^---- .* stdout ----\n(?:.*\n){0,6}", out, re.M)[:2]:
            for x in ln.splitlines():
                log(f"        {x}")
        path.write_text(orig, encoding="utf-8")
        rc, p, f, ok, bad2_, _ = cargo_test(scratch / "core", crate, target)
        green = rc == 0 and f == 0
        log(f"      还原后绿：{crate} passed={p} failed={f}")
        if not (red and green):
            h_ok = False
    step("h", "PASS" if h_ok else "FAIL", f"变异 {len(MUTATIONS)} 处：各自点名的测试变红，还原后重跑变绿")

    # i. codegen
    rc = codegen(scratch, log)
    step("i", "PASS" if rc == 0 else "FAIL", "--codegen：protoc + 钉版本的两个插件，生成物只留在副本")

    # j. go build / vet
    rb = run(["go", "build", "./..."], scratch / "edge")
    rv = run(["go", "vet", "./..."], scratch / "edge")
    msg = f"副本 edge/：go build ./... → exit {rb.returncode}；go vet ./... → exit {rv.returncode}（不跑 go test：main_test.go 的 p0.2 钉归 T0c）"
    step("j", "PASS" if rb.returncode == 0 and rv.returncode == 0 else "FAIL", msg)
    if rb.returncode or rv.returncode:
        log((rb.stderr + rv.stderr)[-3000:])

    # k. 补丁前的二进制 lock --check
    r = run([str(prelock_dir / "aite"), "contracts", "lock", "--check", "--repo", str(scratch)], scratch)
    got_changed = set(re.findall(r"^  changed  (\S+)$", r.stderr, re.M))
    got_added = set(re.findall(r"^  added    (\S+)（新文件未入锁）$", r.stderr, re.M))
    got_deleted = set(re.findall(r"^  deleted  (\S+)", r.stderr, re.M))
    lockface = lambda rel: any(rel.startswith(d + "/") for d in LOCK_DIRS)  # noqa: E731
    exp_added = {rel for rel, _, _ in NEW_FILES if lockface(rel)}
    exp_changed = {e.rel for e in edits if lockface(e.rel)}
    if case == "A":
        exp_changed |= set(P0_CLOSE_LOCKED)
    first = next((l for l in r.stderr.splitlines() if l.startswith("MISMATCH")), "(没有 MISMATCH 行)")
    log(f"    $ /tmp/aite-t0-prelock/aite contracts lock --check --repo {scratch}  → exit {r.returncode}")
    log(f"    {first}")
    for l in r.stderr.splitlines():
        if l.startswith("  changed") or l.startswith("  added") or l.startswith("  deleted"):
            log(f"    {l}")
    want_n = len(exp_changed) + len(exp_added)
    ok_k = (
        r.returncode == 1
        and got_changed == exp_changed
        and got_added == exp_added
        and not got_deleted
        and first == f"MISMATCH {want_n} file(s):"
    )
    step("k", "PASS" if ok_k else "FAIL",
         f"情形 {case}：期望 MISMATCH {want_n}（changed {len(exp_changed)} + added {len(exp_added)}），"
         f"实得 changed {len(got_changed)} + added {len(got_added)} + deleted {len(got_deleted)}，按路径集合{'相等' if ok_k else '不等'}")
    if not ok_k:
        log(f"    多出：{sorted((got_changed | got_added) - (exp_changed | exp_added))}")
        log(f"    缺少：{sorted((exp_changed | exp_added) - (got_changed | got_added))}")

    # l. 数据文件名
    banned = ["pro" + "to", "contracts/", "cla" + "ude", "go.mod", "go.sum", ".lock"]
    bad_names = [p.name for p in DATA_DIR.iterdir() if any(b in p.name for b in banned)]
    step("l", "PASS" if not bad_names else "FAIL",
         f"review/t0/data/ 共 {len(list(DATA_DIR.iterdir()))} 个文件，名字含受保护子串的：{bad_names or '无'}")

    return finish(results)


def finish(results: list[tuple[str, str]]) -> int:
    failed = [t for t, s in results if s == "FAIL"]
    if failed:
        print(f"[T0 self-test] 没过：{', '.join(failed)}")
        return 1
    print("[T0 self-test] 全部通过")
    return 0


# ============================ main ============================================


def main() -> int:
    ap = argparse.ArgumentParser(description="T0：契约 p1.0 补丁")
    ap.add_argument("--check", action="store_true", help="只报告命中与闸门，不写盘")
    ap.add_argument("--root", default=None, help="改哪棵树（默认真仓库根，要授权）")
    ap.add_argument("--self-test", action="store_true", help="在 scratch 副本上跑完整自测")
    ap.add_argument("--scratch", default="/tmp/aite-t0-scratch", help="--self-test 的副本目录（先删后建）")
    ap.add_argument("--codegen", action="store_true", help="在 --root 副本里跑 protoc 生成 edge/gen")
    ap.add_argument("--estimate", action="store_true", help="在 --root 副本上 cargo check，写 review/t0/companion-todo.md")
    args = ap.parse_args()

    if args.self_test:
        scratch = pathlib.Path(args.scratch).resolve()
        if scratch == REPO_ROOT or REPO_ROOT in scratch.parents:
            print("--scratch 不能是真仓库或它里面的目录", file=sys.stderr)
            return 2
        return self_test(scratch)

    root = pathlib.Path(args.root).resolve() if args.root else REPO_ROOT
    if args.codegen or args.estimate:
        if args.root is None or root == REPO_ROOT:
            return refuse_unauthorized("--codegen / --estimate 只对副本跑（要 --root <副本>）")
        if args.codegen:
            return codegen(root)
        return estimate(root, pathlib.Path(__file__).resolve().parent / "companion-todo.md")

    if not authorized(root):
        return refuse_unauthorized("拒绝改真仓库")
    return patch(root, args.check)


if __name__ == "__main__":
    raise SystemExit(main())
