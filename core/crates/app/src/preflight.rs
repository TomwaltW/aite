//! `aite preflight` —— 起飞前自检（对应旧 `scripts/preflight.py`，1211 行）。
//!
//! 七组，挨个点名，每项一行结论 + 非 OK 时一句「怎么补」：
//!
//! | # | 组 | 判据 |
//! |---|---|---|
//! | 1 | 配置可加载 | `config/aite.yaml` 读得出来，**且它起得来**：`worker.system_prompt_path` 指到的文件真的在、`platform` / `model.provider` 的取值不需要注入（配置不存在退到样例，算 WARN；后两者是 FAIL） |
//! | 2 | 环境变量齐 | config 里所有 `*_env` 点到的变量**在不在**（取值一个字都不打）。`platform: fake` 时 SKIP |
//! | 3 | 飞书凭证有效 | 换得到 `tenant_access_token`。`platform: fake` 时 SKIP |
//! | 4 | 飞书身份对得上 | `GET /open-apis/bot/v3/info` 的 `bot.open_id` == `FEISHU_BOT_OPEN_ID`。`platform: fake` 时 SKIP |
//! | 5 | 模型端点通 | 一次最小 chat（`ping`，`max_tokens=16`） |
//! | 6 | 沙箱可用 | 经 edge：daemon 可达 → 起容器 → 四个 import → **一定收掉** |
//! | 7 | 落盘目录可写 | 三个路径的「最近的已存在祖先」写得进去 |
//!
//! **第 1 组为什么不只验「解析得出来」**：2026-09-12 与 2026-09-13 总管连着撞上两次
//! 「preflight 说可以起飞、`aite run` 退出码 2」，病根是同一个 —— 配置解析成功 ≠ 它起得来，
//! 而七组里**没有一组**去问后半句。所以第 1 组管三件事，而不是新开第 8 / 9 组
//! （`docs/dev-spec-2026-09-11-rustgo.md:308` 的「七组自检」是冻结面）：
//!
//! 1. **yaml 解析得出来**（原本就有的那件事）。
//! 2. **`worker.system_prompt_path` 指到的文件真的在**：他那份 `config/aite.yaml` 还指着
//!    当天被删掉的 Python 树（`aite/worker/prompts/`），`require_system_prompt`（`app.rs`）
//!    在起飞时才拦下来。判据直接复用 `aite run` 走的那个 [`load_system_prompt`]，两边是
//!    **逐字同一条路径**；相对路径按 `Options.repo_root` 解析，而它就是进程 cwd（见 [`run`]），
//!    与 `require_system_prompt` 相对 cwd 的口径对得上 —— 不会出现「这边说行、那边说不行」。
//! 3. **`platform` / `model.provider` 的取值不需要注入**：`platform: fake` 与
//!    `model.provider: scripted` 只能被**注入着**用，而 `aite run` 不注入任何实现，
//!    `build_app` 明文拒绝、退出码 2。判据见 [`injection_fault`]，与 `app.rs` 那两条
//!    不等式同源。这一档下第 2/3/4 组一并 SKIP —— fake 平台压根不连飞书，那三组的
//!    判据对它毫无意义（理由写在各自那一行里，见 [`FAKE_PLATFORM_SKIP`]）。
//!
//! 三件事**一次报齐**（口径照第 5 组「缺什么一次报齐」），不是撞上第一条就早退。
//! 它们都不碰网络也不碰 docker，所以 `--offline` 下照样跑 —— 这正是要紧的地方：
//! 上面两种「全绿而起不来」在 `--offline` 下也全绿，第 5/6 组那种「被 offline 跳过」
//! 的口子救不了它们。
//!
//! **红线：任何输出都不得出现密钥取值。** 第一道是代码里根本不去打它们；第二道是
//! [`Redactor`] —— 所有 detail / fix / extra 在渲染前都过一遍，把已知的取值抹掉。
//! 短于 4 个字符的值不替换（那种东西本来也不是密钥，拿一两个字符去全局 replace
//! 会把正常输出打成马赛克）。
//!
//! **一项失败不阻断后面的**：每组都各自收敛成一行结论，连「这一项自己炸了」也是一行。
//! 任一 FAIL → 退出 1。
//!
//! 与 Python 版的唯一结构差异是第 6 组：真沙箱在 edge（Go），core 不认识 Docker，
//! 所以这一组走 `EdgeClient`（`SandboxService` + `EdgeStatusService`）。代价是它要求
//! `aite-edge` 在跑 —— 不在就是这一组 FAIL，并把「先起 aite-edge」写进「怎么补」。
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aite_contracts::{
    AiteConfig, CONTRACT_VERSION, ExecRequest, Message, ModelConfig, ModelProvider, PlatformChoice,
    Role, SandboxError, SandboxErrorKind, SandboxNetwork, SandboxPort, SandboxSpec,
};
use aite_edge_client::{EdgeClient, EdgeStatus};
use aite_models::{OpenAiCompatModel, cost_of, env_snapshot, resolve_api_key};
use serde_json::{Map, Value, json};

use aite_worker::context::load_system_prompt;

use crate::app::{DEFAULT_CONFIG_PATH, load_config, sandbox_spec_of};

const EXAMPLE_CONFIG_PATH: &str = "config/aite.example.yaml";
/// `worker.system_prompt_path` 该长什么样。与 `AiteConfig::default()`（契约）
/// 和 `config/aite.example.yaml` 里那一行同值 —— 三处同时改才会一致，
/// 所以 `prompt_default_matches_the_contract` 拿契约默认值把这个常量钉住了。
const DEFAULT_SYSTEM_PROMPT_PATH: &str = "core/crates/worker/prompts/platform.md";
/// 2026-09-12 删掉的 Python 树里 prompt 的位置。旧配置十有八九还指着它 ——
/// 认出来就能在「怎么补」里直接说破病史，而不是让人自己去猜路径为什么不对。
const DELETED_PYTHON_PROMPT_DIR: &str = "aite/worker/prompts/";

/// `platform: fake` 时第 2/3/4 组那一行的理由。口径照 `--offline：不碰网络` ——
/// 「判据不适用」要写在那一行里，不能默默跳过。
const FAKE_PLATFORM_SKIP: &str = "platform=fake：不连飞书，这一组的判据不适用";

/// 真机起飞该用的 `platform` / `model.provider`。它们只在「怎么补」里露面 ——
/// 就是那句「该改成什么」。契约哪天换了默认值而这两个常量没跟上，自检会一脸笃定地
/// 把人指到一个起不来的取值上，**而且没有任何别的测试会红**（第 1 组照样 FAIL、
/// 照样给 fix，只是那句 fix 是错的）。所以拿契约当唯一真值源钉一次，口径照
/// [`DEFAULT_SYSTEM_PROMPT_PATH`]：`injection_fix_matches_the_contract_defaults`。
const REAL_PLATFORM: &str = "feishu";
const REAL_MODEL_PROVIDER: &str = "openai_compat";

/// 飞书开放平台默认域（与 `edge/internal/feishu` 同值）。
const DEFAULT_DOMAIN: &str = "https://open.feishu.cn";
const PATH_TENANT_TOKEN: &str = "/open-apis/auth/v3/tenant_access_token/internal";
/// 「获取机器人信息」。v3 老接口，响应形状是 `{"code":0,"msg":"ok","bot":{...}}` ——
/// `bot` 在**顶层**而不是 `data` 里（inventory §11 第 34 条）。
const PATH_BOT_INFO: &str = "/open-apis/bot/v3/info";
const PATH_MESSAGES: &str = "/open-apis/im/v1/messages";

/// 网络类检查的超时。起飞前自检要的是「快而诚实」，不是等它慢慢连上。
const FEISHU_TIMEOUT_SEC: u64 = 20;
const MODEL_TIMEOUT_SEC: u64 = 60;

/// 第 5 组发的最小 chat：够拿到一次 usage 就行，别烧钱。
const MODEL_PROBE_PROMPT: &str = "ping";
const MODEL_PROBE_MAX_TOKENS: u32 = 16;

/// 第 6 组在容器里跑的探针。四个 import 是镜像的底线；顺手把版本号带回来。
const SANDBOX_PROBE_CODE: &str = r#"
import importlib, json
out = {}
for name in ("pandas", "matplotlib", "openpyxl", "docx"):
    out[name] = getattr(importlib.import_module(name), "__version__", "?")
print("AITE_PREFLIGHT_OK " + json.dumps(out))
"#;
const SANDBOX_PROBE_MARK: &str = "AITE_PREFLIGHT_OK";
const SANDBOX_PROBE_TIMEOUT_SEC: u32 = 60;

/// 取值短于这个长度的不做脱敏替换 —— 那种东西本来也不是密钥，
/// 而拿一两个字符去全局 replace 会把正常输出打成马赛克。
///
/// 按**字符**数算，不是字节数（[`Redactor::add`]）。这是个取舍：只有 2–3 个字符的
/// 取值此后不再被抹。理由是「2–3 个字符的东西不是密钥」，而按字节算会让两个汉字
/// （6 字节）过闸 —— 于是自检输出里凡出现这两个字都被替换掉，排障最需要说人话的
/// 时刻反被自己的脱敏闸打成马赛克。Python 原版 `len()` 数的就是字符。
const MIN_REDACT_LEN: usize = 4;

/// 标题列宽（按显示宽度，中文算 2）。
const TITLE_WIDTH: usize = 16;
/// 第 4 组之后插 §3.7 的提示行。
const NOTES_AFTER: &str = "feishu_identity";

// --------------------------------------------------------------------------
// 结果模型
// --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Fail,
    Warn,
    Skip,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Fail => "fail",
            Status::Warn => "warn",
            Status::Skip => "skip",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Status::Ok => "OK",
            Status::Fail => "FAIL",
            Status::Warn => "WARN",
            Status::Skip => "SKIP",
        }
    }
}

/// 一组检查的结论。`fix` 只在非 OK 时有意义 —— 每条 FAIL 都要说清怎么补。
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub name: &'static str,
    pub title: &'static str,
    pub status: Status,
    pub detail: String,
    pub fix: String,
    pub extra: Map<String, Value>,
}

impl CheckResult {
    fn new(name: &'static str, title: &'static str, status: Status, detail: String) -> Self {
        Self {
            name,
            title,
            status,
            detail,
            fix: String::new(),
            extra: Map::new(),
        }
    }
    fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = fix.into();
        self
    }
    fn with_extra(mut self, extra: Map<String, Value>) -> Self {
        self.extra = extra;
        self
    }
    fn failed(&self) -> bool {
        self.status == Status::Fail
    }
}

fn ok(name: &'static str, title: &'static str, detail: impl Into<String>) -> CheckResult {
    CheckResult::new(name, title, Status::Ok, detail.into())
}
fn fail(name: &'static str, title: &'static str, detail: impl Into<String>) -> CheckResult {
    CheckResult::new(name, title, Status::Fail, detail.into())
}
fn warn(name: &'static str, title: &'static str, detail: impl Into<String>) -> CheckResult {
    CheckResult::new(name, title, Status::Warn, detail.into())
}
fn skipped(name: &'static str, title: &'static str, reason: impl Into<String>) -> CheckResult {
    CheckResult::new(name, title, Status::Skip, reason.into())
}
fn blocked(name: &'static str, title: &'static str, missing: &[String]) -> CheckResult {
    fail(
        name,
        title,
        format!("前置未满足：{} 未设置，这一项没法查", missing.join(" ")),
    )
    .with_fix("先把第 2 组点名的环境变量补齐再重跑")
}

/// 不进判据、只报事实的提示行（现在只有 §3.7 那两条待核实项）。
#[derive(Debug, Clone)]
pub struct Note {
    pub name: &'static str,
    /// verified | unverified | unverifiable
    pub status: &'static str,
    pub detail: String,
}

pub struct Report {
    pub checks: Vec<CheckResult>,
    pub notes: Vec<Note>,
    pub config_path: String,
    pub offline: bool,
}

impl Report {
    pub fn ok(&self) -> bool {
        !self.checks.iter().any(CheckResult::failed)
    }
    /// 「过了几项」= 没红的项。WARN / SKIP 不拦起飞，所以都算过。
    pub fn passed(&self) -> usize {
        self.checks.iter().filter(|c| !c.failed()).count()
    }
}

// --------------------------------------------------------------------------
// 脱敏
// --------------------------------------------------------------------------

/// 把已知的密钥取值从任何要输出的文本里抹掉。
///
/// 这是「绝不打印密钥」的第二道闸：第一道是代码里根本不去打它们，但错误消息是上游给的
/// （飞书的 msg、reqwest 的 URL、模型端点的响应体），谁也不能保证里面不带凭证。
#[derive(Default)]
pub struct Redactor {
    items: Vec<(String, String)>,
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, value: &str, label: &str) {
        let value = value.trim();
        // 数字符不数字节：`"配置".len()` 是 6，按字节算它就过闸了，于是任何输出里
        // 出现「配置」两个字都会被替换成占位符（见 [`MIN_REDACT_LEN`]）。
        if value.chars().count() < MIN_REDACT_LEN || self.items.iter().any(|(v, _)| v == value) {
            return;
        }
        self.items.push((value.to_string(), label.to_string()));
        // 长的先替换，免得短取值恰好是长取值的前缀时替出半截。
        self.items
            .sort_by_key(|item| std::cmp::Reverse(item.0.len()));
    }

    pub fn scrub(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (value, label) in &self.items {
            out = out.replace(value, &format!("«{label} 的取值已隐去»"));
        }
        out
    }

    /// 递归洗 `extra` 这种嵌套结构。
    ///
    /// 不走「to_string → 字符串替换 → from_str」那条近路：取值里只要有引号或反斜杠，
    /// 序列化会把它转义掉，字符串替换就对不上，脱敏静默失效。
    pub fn scrub_value(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.scrub(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.scrub_value(x)).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, x)| (k.clone(), self.scrub_value(x)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

// --------------------------------------------------------------------------
// 小工具
// --------------------------------------------------------------------------

/// 把 `source()` 链一路拼进消息 —— 不拼的话三种完全不同的网络病打出来一模一样。
///
/// `reqwest::Error` 的 `Display` 只写 kind + url，真正的原因（连接被拒 / DNS 解析不了 /
/// TLS 被中间设备换掉 / 代理不通）全在链上。实测：`HTTPS_PROXY=http://127.0.0.1:9`
/// （端口关着）和 `HTTPS_PROXY=http://no-such-proxy-host.invalid:8080`（主机名解析不了）
/// 这两种病，改之前打出来逐字相同：
/// `换 tenant_access_token 失败：error sending request for url (https://open.feishu.cn/...)`。
/// 总管拿到这一行，不知道该去查代理、查 DNS 还是查公司的 TLS 拦截 ——
/// 而 `docs/acceptance-M.md` §0.1 说的是「任一 FAIL 就别往下走」，这一行就是他唯一的线索。
///
/// 链尾才是根因，所以拼在后面：[`tail`] 截的也是尾巴，截完根因还在。
fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        let msg = s.to_string();
        // 上游常把下一层的 Display 原样嵌进自己的消息里（hyper / rustls 都这么干），
        // 拼两遍只会更难读。
        if !msg.is_empty() && !out.contains(&msg) {
            out.push_str(" ← ");
            out.push_str(&msg);
        }
        cur = s.source();
    }
    out
}

/// 错误输出只留尾巴 —— 自检行是给人一眼扫的，完整栈去看日志。
fn tail(text: &str, limit: usize) -> String {
    let squeezed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = squeezed.chars().collect();
    if chars.len() <= limit {
        squeezed
    } else {
        format!(
            "…{}",
            chars[chars.len() - limit..].iter().collect::<String>()
        )
    }
}

/// 把 config 里的相对路径按 `repo_root` 解析成绝对路径。
///
/// `repo_root` 在真跑时就是进程 cwd（[`run`] 里 `current_dir()`），所以这条和
/// `require_system_prompt` / `load_config` 那边「相对进程工作目录」的口径是同一条。
fn resolve_under(repo_root: &Path, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        repo_root.join(p)
    }
}

/// 第 1 组因为 system prompt 而 FAIL 时那句「怎么补」。
///
/// 三件事必须都在里面：**当前值**是什么、**应该是**什么、以及 2026-09-12 那条真实病史。
/// 总管撞上的就是「旧配置还指着已删的 Python 树」那一种 —— 不说破的话他只看见一个
/// 路径不存在，猜不到是哪次改动把它搬走的，更猜不到该往哪儿改。
fn system_prompt_fix(raw: &str, abs: &Path) -> String {
    let mut out = format!(
        "把配置里的 worker.system_prompt_path 改成 {DEFAULT_SYSTEM_PROMPT_PATH}\
         （{EXAMPLE_CONFIG_PATH} 里就是这个值）；当前值 {raw} 解析成 {}，那儿没有这个文件",
        abs.display()
    );
    if raw.contains(DELETED_PYTHON_PROMPT_DIR) {
        out.push_str(
            " —— 它指的是 2026-09-12 删掉的 Python 树，\
             aite/worker/prompts/ 现在已经不存在了，2026-09-10 之前写的配置都要改这一行",
        );
    } else {
        out.push_str("；相对路径按进程的工作目录算，顺带确认一下起进程时的 cwd");
    }
    out
}

/// 第 ① 组的第三件事：`platform` / `model.provider` 的取值跟「有没有注入」搭不搭。
///
/// 返回 `(那一行说什么, 怎么补)`；两个取值都能自己起飞时返回 `None`。
///
/// **preflight 凭什么断定「没有注入」**：`Injections` 是 `build_app` 的入参，preflight
/// 手上根本没有这个东西 —— 但它也不需要有。preflight 体检的对象是 `aite run`，而
/// `aite run` 那条路（`cli.rs` 里唯一那个 `build_app` 调用点）传的是
/// `Injections::default()`，**三个口子全空、也没有任何开关能往里塞东西**。所以在
/// preflight 的语境里「有没有注入」不是未知数，是一个定值：没有，且不可能有。
/// 这条前提由 `tests/preflight_e2e.rs` 的 `build_app_really_refuses_what_the_first_check_refuses`
/// 拿真 `build_app` 对拍钉着 —— 哪天真给 `aite run` 加了注入开关，那条会红。
///
/// 判据与 `app.rs` 逐字同源：平台那条是 `config.platform == Fake`（`build_app` 第 3 步），
/// 模型那条是 `provider != OpenaiCompat`（`build_model`）—— **照抄它的不等式而不是写死
/// `== Scripted`**，契约哪天加第三个 provider，这里自动跟上。
///
/// 与第 5 组不重复：第 5 组管「端点通不通」，`provider=scripted` 时那件事无从谈起，
/// 所以它给 WARN；「这份配置起不起得来」是第 ① 组的本分，所以这里是 FAIL。
fn injection_fault(cfg: &AiteConfig) -> Option<(String, String)> {
    let mut needs: Vec<String> = Vec::new();
    let mut fixes: Vec<String> = Vec::new();

    if cfg.platform == PlatformChoice::Fake {
        needs.push("platform=fake 要由调用方注入平台实现".to_string());
        fixes.push(format!(
            "真机起飞把配置里的 platform 改成 {REAL_PLATFORM}（{EXAMPLE_CONFIG_PATH} 里就是这个值）；\
             当前值 fake 是留给评测 / 回放的取值（§3.1），不会去连真实飞书 —— \
             评测走 `aite evals --platform fake`，那条路自己注入替身，不经过 aite run"
        ));
    }
    if cfg.model.provider != ModelProvider::OpenaiCompat {
        needs.push(format!(
            "model.provider={} 要由调用方注入模型实现",
            cfg.model.provider
        ));
        fixes.push(format!(
            "真机起飞把配置里的 model.provider 改成 {REAL_MODEL_PROVIDER}（{EXAMPLE_CONFIG_PATH} 里就是这个值）；\
             当前值 {} 是留给评测的取值（§3.1），没有真端点 —— \
             评测走 `aite evals --model scripted`，那条路自己注入替身，不经过 aite run",
            cfg.model.provider
        ));
    }

    if needs.is_empty() {
        return None;
    }
    Some((
        format!(
            "{} —— aite run 不注入任何实现，起飞会被拒（退出码 2）",
            needs.join("、")
        ),
        // 与 `check_config` 拼 `faults` / `fixes` 用同一个连接词：三件事全犯的配置，
        // 「怎么补」读起来是三条并列的指令，而不是一句话的延续。
        fixes.join("；另外，"),
    ))
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| if (c as u32) > 0x2e80 { 2 } else { 1 })
        .sum()
}

fn pad(text: &str, width: usize) -> String {
    let w = display_width(text);
    if w >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - w))
    }
}

/// 扫出 config 里所有 `*_env` 字段 → `[(字段路径, 环境变量名)]`。
///
/// generic 地扫而不是写死那四个名字：契约以后再加一个 `*_env`，这里自动跟上，
/// 不会出现「新加的凭证自检查不到」这种静默盲区。走 serde 的 JSON 形态，
/// 字段名与 yaml 里的键逐字一致。
pub fn env_var_names(cfg: &AiteConfig) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(value) = serde_json::to_value(cfg) else {
        return out;
    };
    fn walk(v: &Value, prefix: &str, out: &mut Vec<(String, String)>) {
        if let Value::Object(map) = v {
            for (k, val) in map {
                match val {
                    Value::Object(_) => walk(val, &format!("{prefix}{k}."), out),
                    Value::String(s) if k.ends_with("_env") && !s.is_empty() => {
                        out.push((format!("{prefix}{k}"), s.clone()));
                    }
                    _ => {}
                }
            }
        }
    }
    walk(&value, "", &mut out);
    out
}

/// 把名字表点到的每个环境变量的取值都交给 [`Redactor`]。
///
/// 吃的是 `[(字段路径, 变量名)]` 而不是 `&AiteConfig`，因为名字表的**唯一**来源是
/// [`env_var_names`] 那套泛化扫描。写死那四个字段的话，契约哪天加第 5 个 `*_env`，
/// 第 2 组会自动报它「在不在」，而 [`Redactor`] 永远不认识它的取值 —— 脱敏静默漏，
/// 且没有任何一条测试会红。
fn arm_redactor(
    redactor: &mut Redactor,
    names: &[(String, String)],
    env: &HashMap<String, String>,
) {
    for (_field, var) in names {
        if let Some(value) = env.get(var) {
            redactor.add(value, var);
        }
    }
}

// --------------------------------------------------------------------------
// 1 配置可加载
// --------------------------------------------------------------------------

fn check_config(
    path: &Path,
    fell_back: bool,
    repo_root: &Path,
) -> (CheckResult, Option<AiteConfig>) {
    let cfg = match load_config(path) {
        Ok(c) => c,
        Err(e) => {
            return (
                fail("config", "配置可加载", tail(&e.to_string(), 300)).with_fix(format!(
                    "照 {EXAMPLE_CONFIG_PATH} 的形状修 {}；密钥只写环境变量名，不写取值",
                    path.display()
                )),
                None,
            );
        }
    };
    let detail = format!(
        "platform={} · model.provider={} · sandbox.image={}",
        cfg.platform, cfg.model.provider, cfg.sandbox.image
    );
    let mut extra = Map::new();
    extra.insert("platform".into(), json!(cfg.platform.as_str()));
    extra.insert("model_provider".into(), json!(cfg.model.provider.as_str()));
    extra.insert("sandbox_image".into(), json!(cfg.sandbox.image));
    extra.insert("config_path".into(), json!(path.display().to_string()));

    // 配置解析成功 ≠ 它起得来。下面两件事都是**硬起飞前提**（`build_app` 拒绝起飞的
    // 原文就是它们），所以都是 FAIL 不是 WARN —— 见模块头「第 1 组为什么不只验解析」。
    //
    // **一次报齐，不是撞上第一条就早退**：口径照第 5 组那句「缺什么一次报齐，别让人补完
    // base_url 重跑一遍才发现还缺 key」。两条都犯了的配置，一轮就能全改完。
    // 只命中一条时这一行与「只验 prompt」那天**逐字相同**（涟漪最小）。
    let mut faults: Vec<String> = Vec::new();
    let mut fixes: Vec<String> = Vec::new();

    // 其一：`worker.system_prompt_path` 指到的文件真的在（`build_app` 第 2 步
    // `require_system_prompt`）。
    let prompt_raw = cfg.worker.system_prompt_path.clone();
    let prompt_abs = resolve_under(repo_root, &prompt_raw);
    extra.insert("system_prompt_path".into(), json!(prompt_raw));
    extra.insert(
        "system_prompt_resolved".into(),
        json!(prompt_abs.display().to_string()),
    );
    match load_system_prompt(&prompt_abs) {
        Err(e) => {
            extra.insert("system_prompt_ok".into(), json!(false));
            // 上一句已经把绝对路径打全了，`WorkerError` 的消息里还会再带一遍 —— 不存在这种
            // 最常见的形态只说两个字，把版面留给「怎么补」。别的（是个目录、没读权限）
            // 才需要上游的原文，那时候绝对路径重复一次也认了。
            let why = if prompt_abs.exists() {
                tail(&e.to_string(), 120)
            } else {
                "不存在".to_string()
            };
            faults.push(format!(
                "worker.system_prompt_path 指不到文件：{prompt_raw} → {}（{why}）",
                prompt_abs.display(),
            ));
            fixes.push(system_prompt_fix(&prompt_raw, &prompt_abs));
        }
        Ok(_) => {
            extra.insert("system_prompt_ok".into(), json!(true));
        }
    }

    // 其二：`platform` / `model.provider` 的取值自己起得来（`build_app` 第 3 步与
    // `build_model` 那两条明文拒绝）。见 [`injection_fault`]。
    match injection_fault(&cfg) {
        Some((why, fix)) => {
            extra.insert("needs_injection".into(), json!(true));
            faults.push(why);
            fixes.push(fix);
        }
        None => {
            extra.insert("needs_injection".into(), json!(false));
        }
    }

    if !faults.is_empty() {
        return (
            fail(
                "config",
                "配置可加载",
                format!("{detail} · 但 {}", faults.join("；另外，")),
            )
            .with_fix(fixes.join("；另外，"))
            // FAIL 也把 cfg 交出去：「一项失败不阻断后面的」，后面六组照常跑完。
            .with_extra(extra),
            Some(cfg),
        );
    }

    if fell_back {
        // 样例配置能过形状校验，但 base_url / model 是空的，真机起飞用它必炸。
        // 不算 FAIL（CI 和这台机器上本来就只有样例），但必须显式说出来。
        extra.insert("fell_back_to_example".into(), json!(true));
        return (
            warn(
                "config",
                "配置可加载",
                format!("{DEFAULT_CONFIG_PATH} 不存在，退到样例 {EXAMPLE_CONFIG_PATH} · {detail}"),
            )
            .with_fix(format!(
                "起飞前 `cp {EXAMPLE_CONFIG_PATH} {DEFAULT_CONFIG_PATH}` 并填上 model.base_url / model.model"
            ))
            .with_extra(extra),
            Some(cfg),
        );
    }
    (
        ok("config", "配置可加载", detail).with_extra(extra),
        Some(cfg),
    )
}

// --------------------------------------------------------------------------
// 2 环境变量齐
// --------------------------------------------------------------------------

/// 只报「在不在」。取值一个字都不出现在这里 —— 红线第一条。
fn check_env(cfg: &AiteConfig, env: &HashMap<String, String>, offline: bool) -> CheckResult {
    let names = env_var_names(cfg);
    let isset = |var: &String| env.get(var).map(|v| !v.trim().is_empty()).unwrap_or(false);
    let missing: Vec<String> = names
        .iter()
        .filter(|(_f, v)| !isset(v))
        .map(|(_f, v)| v.clone())
        .collect();
    let present: Vec<String> = names
        .iter()
        .filter(|(_f, v)| isset(v))
        .map(|(_f, v)| v.clone())
        .collect();
    let mut extra = Map::new();
    extra.insert(
        "required".into(),
        json!(names.iter().map(|(_f, v)| v.clone()).collect::<Vec<_>>()),
    );
    extra.insert("missing".into(), json!(missing));
    extra.insert("present".into(), json!(present));

    if missing.is_empty() {
        return ok(
            "env",
            "环境变量齐",
            format!("{} 个都已设置：{}", names.len(), present.join(" ")),
        )
        .with_extra(extra);
    }
    let mut detail = format!(
        "{}/{} 个未设置：{}",
        missing.len(),
        names.len(),
        missing.join(" ")
    );
    if !present.is_empty() {
        detail.push_str(&format!("（已设置：{}）", present.join(" ")));
    }
    if offline {
        // `--offline` 就是给 CI 和没凭证的机器用的，在这儿把缺变量判成 FAIL
        // 等于说「CI 上永远跑不过」。降成 WARN，但一个名字都不少报。
        extra.insert("offline_downgraded".into(), json!(true));
        return warn(
            "env",
            "环境变量齐",
            format!("{detail} —— --offline 下不作判据"),
        )
        .with_fix("真机起飞前去掉 --offline 重跑一次，这几项必须是 OK")
        .with_extra(extra);
    }
    let exports: Vec<String> = missing.iter().map(|v| format!("{v}=…")).collect();
    fail("env", "环境变量齐", detail)
        .with_fix(format!(
            "export {}；飞书三项去开放平台 → 应用 → 凭证与基础信息 / 机器人，模型密钥去模型厂商控制台。\
             取值只放环境变量，不要写进 {DEFAULT_CONFIG_PATH}",
            exports.join(" ")
        ))
        .with_extra(extra)
}

// --------------------------------------------------------------------------
// 3 / 4 飞书：凭证有效 + 身份对得上（顺带 §3.7(b) 的探测）
// --------------------------------------------------------------------------

fn note_passive_listen() -> Note {
    Note {
        name: "feishu_passive_listen",
        status: "unverifiable",
        detail: "§3.7(a) 只有 @ 权限时话题内不带 @ 的回复是否投递 —— 未核实，且起飞前查不了：\
                 要真在话题里发一条不带 @ 的消息、看事件有没有投递才知道，M4 就是那个实验。\
                 当前 FEISHU_P0.supports_passive_listen=False（保守取值），M4 请带 @ 先走通。"
            .to_string(),
    }
}

fn note_history_scope_unverified() -> Note {
    Note {
        name: "feishu_group_history_scope",
        status: "unverified",
        detail: "§3.7(b) 群历史是否要「获取群组中所有消息」敏感权限 —— 未核实。\
                 飞书没有「列出本应用已授权范围」的免权限接口，光靠凭证问不出来；\
                 带 --chat-id <测试群 chat_id> 重跑，我就直接调一次群历史给你结论（M5 靠它）。"
            .to_string(),
    }
}

struct FeishuOutcome {
    token: CheckResult,
    identity: CheckResult,
    notes: Vec<Note>,
}

/// 第 3 组（换 token）和第 4 组（身份比对）共用一条连接，一起跑。
///
/// 第 4 组为什么重要：app_id/app_secret 和 `FEISHU_BOT_OPEN_ID` 来自不同应用时，
/// token 照样换得到、进程照样起得来，但 @ 识别永远匹配不上 —— M1 表现为「静默不响应」。
async fn check_feishu(
    cfg: &AiteConfig,
    env: &HashMap<String, String>,
    redactor: &mut Redactor,
    chat_id: Option<&str>,
    domain: &str,
) -> FeishuOutcome {
    let pick = |name: &str| {
        env.get(name)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let app_id = pick(&cfg.feishu.app_id_env);
    let app_secret = pick(&cfg.feishu.app_secret_env);
    let want_open_id = pick(&cfg.feishu.bot_open_id_env);
    redactor.add(&app_id, &cfg.feishu.app_id_env);
    redactor.add(&app_secret, &cfg.feishu.app_secret_env);
    redactor.add(&want_open_id, &cfg.feishu.bot_open_id_env);

    let mut missing = Vec::new();
    if app_id.is_empty() {
        missing.push(cfg.feishu.app_id_env.clone());
    }
    if app_secret.is_empty() {
        missing.push(cfg.feishu.app_secret_env.clone());
    }
    if !missing.is_empty() {
        return FeishuOutcome {
            token: blocked("feishu_token", "飞书凭证有效", &missing),
            identity: blocked("feishu_identity", "飞书身份对得上", &missing),
            notes: vec![note_passive_listen(), note_history_scope_unverified()],
        };
    }

    let http = match reqwest::Client::builder()
        .timeout(Duration::from_secs(FEISHU_TIMEOUT_SEC))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return FeishuOutcome {
                token: fail(
                    "feishu_token",
                    "飞书凭证有效",
                    format!("HTTP 客户端建不起来：{}", tail(&error_chain(&e), 300)),
                )
                .with_fix("这行连同上面的输出一起报告 —— 不是环境的问题"),
                identity: skipped(
                    "feishu_identity",
                    "飞书身份对得上",
                    "第 3 组没过，身份无从比对",
                ),
                notes: vec![note_passive_listen(), note_history_scope_unverified()],
            };
        }
    };

    let domain = domain.trim_end_matches('/');
    let (token_result, token) =
        check_feishu_token(&http, domain, &app_id, &app_secret, redactor).await;
    let Some(token) = token else {
        return FeishuOutcome {
            token: token_result,
            identity: skipped(
                "feishu_identity",
                "飞书身份对得上",
                "第 3 组没过，身份无从比对",
            ),
            notes: vec![note_passive_listen(), note_history_scope_unverified()],
        };
    };
    let identity = check_feishu_identity(&http, domain, &token, &want_open_id, cfg).await;
    let scope = match chat_id {
        Some(id) => probe_history_scope(&http, domain, &token, id).await,
        None => note_history_scope_unverified(),
    };
    FeishuOutcome {
        token: token_result,
        identity,
        notes: vec![note_passive_listen(), scope],
    }
}

async fn check_feishu_token(
    http: &reqwest::Client,
    domain: &str,
    app_id: &str,
    app_secret: &str,
    redactor: &mut Redactor,
) -> (CheckResult, Option<String>) {
    let started = Instant::now();
    let resp = http
        .post(format!("{domain}{PATH_TENANT_TOKEN}"))
        .json(&json!({"app_id": app_id, "app_secret": app_secret}))
        .send()
        .await;
    let body: Value = match resp {
        Ok(r) => match r.json().await {
            Ok(v) => v,
            Err(e) => {
                return (
                    fail(
                        "feishu_token",
                        "飞书凭证有效",
                        format!(
                            "换 tenant_access_token 的响应读不懂：{}",
                            tail(&error_chain(&e), 300)
                        ),
                    )
                    .with_fix("确认 base 域名是 open.feishu.cn，本机没有被代理改写响应"),
                    None,
                );
            }
        },
        Err(e) => {
            return (
                fail(
                    "feishu_token",
                    "飞书凭证有效",
                    format!(
                        "换 tenant_access_token 失败：{}",
                        tail(&error_chain(&e), 300)
                    ),
                )
                .with_fix("先确认本机能出网访问 open.feishu.cn，再核对两个凭证环境变量"),
                None,
            );
        }
    };
    let code = body.get("code").and_then(Value::as_i64).unwrap_or(-1);
    let token = body
        .get("tenant_access_token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if code != 0 || token.is_empty() {
        let msg = body
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        return (
            fail(
                "feishu_token",
                "飞书凭证有效",
                format!(
                    "换 tenant_access_token 失败：code={code} msg={}",
                    tail(&msg, 300)
                ),
            )
            .with_fix(
                "核对 FEISHU_APP_ID / FEISHU_APP_SECRET 是不是同一个自建应用的凭证\
                 （开放平台 → 应用 → 凭证与基础信息）；连不上则先看本机出网",
            ),
            None,
        );
    }
    // token 本身是密钥，进脱敏表；后面任何错误消息里再出现它都会被抹掉。
    redactor.add(&token, "tenant_access_token");
    let elapsed = started.elapsed().as_millis() as u64;
    let mut extra = Map::new();
    extra.insert("latency_ms".into(), json!(elapsed));
    (
        ok(
            "feishu_token",
            "飞书凭证有效",
            format!("tenant_access_token 换到了（{elapsed}ms，取值不打印）"),
        )
        .with_extra(extra),
        Some(token),
    )
}

async fn check_feishu_identity(
    http: &reqwest::Client,
    domain: &str,
    token: &str,
    want_open_id: &str,
    cfg: &AiteConfig,
) -> CheckResult {
    let title = "飞书身份对得上";
    let resp = http
        .get(format!("{domain}{PATH_BOT_INFO}"))
        .bearer_auth(token)
        .send()
        .await;
    let (http_status, body): (u16, Value) = match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            let body = r.json().await.unwrap_or(Value::Null);
            (status, body)
        }
        Err(e) => {
            return fail(
                "feishu_identity",
                title,
                format!("查机器人信息失败：{}", tail(&error_chain(&e), 300)),
            )
            .with_fix(format!("确认应用开了机器人能力；接口 GET {PATH_BOT_INFO}"));
        }
    };
    let code = body
        .get("code")
        .and_then(Value::as_i64)
        .unwrap_or(http_status as i64);
    if code != 0 {
        let msg = body.get("msg").and_then(Value::as_str).unwrap_or_default();
        return fail(
            "feishu_identity",
            title,
            format!(
                "查机器人信息失败：http={http_status} code={code} msg={}",
                tail(msg, 300)
            ),
        )
        .with_fix(
            "多半是应用没开机器人能力，或权限没批 —— 对照 §3.7 的权限清单：\
             接收群聊中 @ 机器人消息 / 读取群历史消息 / 发送与更新消息与卡片 / 上传下载文件 / 消息表情回复",
        );
    }
    // `bot` 在顶层而不是 data 里（v3 老接口）。
    let bot = body.get("bot").cloned().unwrap_or(Value::Null);
    let got_open_id = bot
        .get("open_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let app_name = bot
        .get("app_name")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    // activate_status 原样带出，不做数字→含义的解释：猜一个映射写进自检输出比不写更害人。
    let activate = bot.get("activate_status").cloned().unwrap_or(Value::Null);
    let mut extra = Map::new();
    extra.insert("app_name".into(), json!(app_name));
    extra.insert("activate_status".into(), activate.clone());
    extra.insert("bot_open_id_matches".into(), Value::Null);

    if got_open_id.is_empty() {
        return fail(
            "feishu_identity",
            title,
            format!("响应里没有 bot.open_id（app_name={app_name}）"),
        )
        .with_fix(format!(
            "确认应用开了机器人能力；接口 GET {PATH_BOT_INFO} 的响应应含 bot.open_id"
        ))
        .with_extra(extra);
    }
    if want_open_id.is_empty() {
        return fail(
            "feishu_identity",
            title,
            format!(
                "飞书侧机器人取到了（app_name={app_name}），但 {} 未设置，没法比对",
                cfg.feishu.bot_open_id_env
            ),
        )
        .with_fix(format!(
            "export {}=<开放平台 → 应用 → 机器人 页面上该机器人的 open_id>；\
             缺了它 @ 识别永远匹配不上，M1 会静默不响应",
            cfg.feishu.bot_open_id_env
        ))
        .with_extra(extra);
    }
    if got_open_id != want_open_id {
        // 两个取值一个都不打 —— 红线在这儿最容易破，因为「打出来才好查」的诱惑最大。
        extra.insert("bot_open_id_matches".into(), json!(false));
        return fail(
            "feishu_identity",
            title,
            format!(
                "对不上：飞书侧机器人是 {app_name}，与 {} 不是同一个（取值都不打印）",
                cfg.feishu.bot_open_id_env
            ),
        )
        .with_fix(format!(
            "{}/{} 与 {} 多半来自两个不同的应用；到开放平台上认准同一个应用，重取凭证和机器人 open_id",
            cfg.feishu.app_id_env, cfg.feishu.app_secret_env, cfg.feishu.bot_open_id_env
        ))
        .with_extra(extra);
    }
    extra.insert("bot_open_id_matches".into(), json!(true));
    ok(
        "feishu_identity",
        title,
        format!("一致 · app_name={app_name} · activate_status={activate}（取值含义见开放平台文档，此处不解释）"),
    )
    .with_extra(extra)
}

/// §3.7(b) 的实测：拿真 chat_id 调一次群历史，读得到就是读得到。
async fn probe_history_scope(
    http: &reqwest::Client,
    domain: &str,
    token: &str,
    chat_id: &str,
) -> Note {
    // query 串自己拼：reqwest 这边没开 serde_urlencoded 那个 feature（依赖表是冻结的），
    // 而 chat_id 是飞书的 id（`oc_` + 十六进制），百分号编码只要管住非 unreserved 字符。
    let url = format!(
        "{domain}{PATH_MESSAGES}?container_id_type=chat&container_id={}&page_size=1",
        percent_encode(chat_id)
    );
    let resp = http.get(url).bearer_auth(token).send().await;
    let body: Value = match resp {
        Ok(r) => r.json().await.unwrap_or(Value::Null),
        Err(e) => {
            return Note {
                name: "feishu_group_history_scope",
                status: "unverified",
                detail: format!("§3.7(b) 探测没跑成：{}", tail(&error_chain(&e), 300)),
            };
        }
    };
    let code = body.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let msg = body.get("msg").and_then(Value::as_str).unwrap_or_default();
        return Note {
            name: "feishu_group_history_scope",
            status: "verified",
            detail: format!(
                "§3.7(b) 实测：这套凭证读**不到**群历史（code={code} msg={}）。\
                 去开放平台补权限（读取群历史消息；若提示需要「获取群组中所有消息」则它就是必需的敏感权限，\
                 要走审核），补完重跑本项。M5「汇总本群本周开放事项」在此之前一定过不了。",
                tail(msg, 300)
            ),
        };
    }
    let items = body
        .get("data")
        .and_then(|d| d.get("items"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    Note {
        name: "feishu_group_history_scope",
        status: "verified",
        detail: format!(
            "§3.7(b) 实测：这套凭证**读得到**群历史（chat 返回 {items} 条，只取了 1 页 1 条）。\
             也就是说当前已授予的权限足够 M5；至于是不是「获取群组中所有消息」那条在起作用，\
             接口不回权限来源，问不出来 —— 但对起飞而言结论已经够用。"
        ),
    }
}

// --------------------------------------------------------------------------
// 5 模型端点通
// --------------------------------------------------------------------------

async fn check_model(
    cfg: &AiteConfig,
    env: &HashMap<String, String>,
    redactor: &mut Redactor,
) -> CheckResult {
    let title = "模型端点通";
    let mcfg: &ModelConfig = &cfg.model;
    if let Some(v) = env.get(&mcfg.api_key_env) {
        redactor.add(v, &mcfg.api_key_env);
    }

    if mcfg.provider != ModelProvider::OpenaiCompat {
        return warn(
            "model",
            title,
            format!(
                "model.provider={}，不是 openai_compat，没有真端点可探",
                mcfg.provider
            ),
        )
        .with_fix("真机起飞要把 model.provider 设回 openai_compat");
    }

    // 缺什么一次报齐，别让人补完 base_url 重跑一遍才发现还缺 key。
    let mut blanks: Vec<String> = Vec::new();
    if mcfg.base_url.trim().is_empty() {
        blanks.push("model.base_url".to_string());
    }
    if mcfg.model.trim().is_empty() {
        blanks.push("model.model".to_string());
    }
    let api_key = resolve_api_key(mcfg, env).unwrap_or_default();
    if api_key.is_empty() {
        blanks.push(format!("环境变量 {}", mcfg.api_key_env));
    }
    if !blanks.is_empty() {
        let mut fixes: Vec<String> = Vec::new();
        if mcfg.base_url.trim().is_empty() {
            fixes.push("model.base_url 填百炼 / 智谱的 OpenAI 兼容端点".to_string());
        }
        if mcfg.model.trim().is_empty() {
            fixes.push("model.model 填模型名".to_string());
        }
        if api_key.is_empty() {
            fixes.push(format!(
                "export {}=<模型厂商控制台里的 API Key>（只放环境变量）",
                mcfg.api_key_env
            ));
        }
        let mut extra = Map::new();
        extra.insert("missing".into(), json!(blanks));
        return fail(
            "model",
            title,
            format!("配置不全，缺：{}", blanks.join(" / ")),
        )
        .with_fix(fixes.join("；"))
        .with_extra(extra);
    }
    redactor.add(&api_key, &mcfg.api_key_env);

    let model = match OpenAiCompatModel::from_config(mcfg, env) {
        Ok(m) => m,
        Err(e) => {
            return fail("model", title, tail(&error_chain(&e), 300))
                .with_fix("按上面那句把 config 的 model 段补齐");
        }
    };
    let started = Instant::now();
    let messages = vec![Message::text(Role::User, MODEL_PROBE_PROMPT)];
    let turn = match tokio::time::timeout(
        Duration::from_secs(MODEL_TIMEOUT_SEC),
        aite_contracts::ModelPort::chat(
            &model,
            &messages,
            &[],
            MODEL_PROBE_MAX_TOKENS,
            mcfg.temperature,
        ),
    )
    .await
    {
        Err(_) => {
            return fail(
                "model",
                title,
                format!("{MODEL_TIMEOUT_SEC}s 内没回 —— 端点不通或太慢"),
            )
            .with_fix(format!(
                "核对 model.base_url（现在是 {}）能不能从本机访问，以及是否需要走代理",
                mcfg.base_url
            ));
        }
        Ok(Err(e)) => {
            return fail("model", title, tail(&error_chain(&e), 300)).with_fix(format!(
                "401/403 → 换 {} 的取值；404 → 核对 model.base_url 结尾是否要带 /v1、\
                 model.model={} 这个模型名在该厂商是否存在",
                mcfg.api_key_env, mcfg.model
            ));
        }
        Ok(Ok(t)) => t,
    };

    let elapsed = started.elapsed().as_millis() as u64;
    let usage = &turn.usage;
    let cost = cost_of(usage, mcfg);
    let priced = mcfg.price_in_per_mtok != 0.0 || mcfg.price_out_per_mtok != 0.0;
    let money = if priced {
        format!("¥{cost:.6}")
    } else {
        "¥0（config 里 price_in/out_per_mtok 都是 0，没配价格）".to_string()
    };
    let mut extra = Map::new();
    extra.insert("latency_ms".into(), json!(elapsed));
    extra.insert("input_tokens".into(), json!(usage.input_tokens));
    extra.insert("output_tokens".into(), json!(usage.output_tokens));
    extra.insert("cached_tokens".into(), json!(usage.cached_tokens));
    // 9 位而不是显示用的 6 位：一次最小 chat 只花几微元，6 位一舍就没了。
    extra.insert("cost_cny".into(), json!((cost * 1e9).round() / 1e9));
    extra.insert("finish_reason".into(), json!(turn.finish_reason));
    ok(
        "model",
        title,
        format!(
            "{} 通了 · {elapsed}ms · in={} out={} tokens · 本次 {money} · finish={}",
            mcfg.model, usage.input_tokens, usage.output_tokens, turn.finish_reason
        ),
    )
    .with_extra(extra)
}

// --------------------------------------------------------------------------
// 6 沙箱可用（经 edge）
// --------------------------------------------------------------------------

/// daemon → 起容器 → 跑四个 import → **一定收掉**。
///
/// 与 Python 版的差异：真沙箱在 edge（Go），所以 daemon / 镜像都不是 core 直接问的 ——
/// daemon 看 `EdgeStatus.sandbox_ok`，镜像与「四个 import」合并成「真起一个容器跑一遍探针」。
/// 「收干净了没有」只认 [`release`](aite_contracts::SandboxService::release) 自己的回答：
/// 收尾之后那发 `list_files` 只能证明**edge 的记账清了**，证不了容器没了 ——
/// `edge/internal/sandbox/docker.go` 的 `Release` 是先 `delete(d.boxes, id)` 再 `ContainerRemove`，
/// 后者失败时记账早没了，`list_files` 照样回 `not_found`。所以它降级成纯 extra
/// （`sandbox_gone`），不进判据（Python 那边是按 `aite.task` 标签回扫**真容器** ——
/// 那是 Docker 级查询，core 这侧没有对应 RPC）。
async fn check_sandbox(cfg: &AiteConfig, repo_root: &Path) -> CheckResult {
    let title = "沙箱可用";
    let mut extra = Map::new();
    extra.insert("image".into(), json!(cfg.sandbox.image));
    extra.insert(
        "edge_socket".into(),
        json!(repo_root.join(&cfg.edge.edge_socket).display().to_string()),
    );

    let edge = match EdgeClient::connect(&cfg.edge, repo_root).await {
        Ok(c) => Arc::new(c),
        Err(e) => {
            return fail(
                "sandbox",
                title,
                format!("连不上 edge：{}", tail(&error_chain(&e), 300)),
            )
            .with_fix("先起 `aite-edge --config <配置>`（它才是认识 Docker 的那一侧）")
            .with_extra(extra);
        }
    };
    let status = match edge.status().await {
        Ok(s) => s,
        Err(e) => {
            return fail(
                "sandbox",
                title,
                format!(
                    "edge 不应答（{}）：{}",
                    repo_root.join(&cfg.edge.edge_socket).display(),
                    tail(&error_chain(&e), 300)
                ),
            )
            .with_fix("先起 `aite-edge --config <配置>`；起着的话看它的日志为什么不应答")
            .with_extra(extra);
        }
    };
    if let Some(verdict) = edge_status_verdict(title, &status, &mut extra) {
        return verdict;
    }
    probe_sandbox(&*edge.sandbox(), cfg, extra).await
}

/// `EdgeStatus` 上那两条判据：两边契约版本要一致、docker daemon 要可达。
///
/// 抽出来是为了不起真 edge 就能造出「daemon 挂了」的现场（Python 那边的
/// `test_docker_daemon_down_is_one_fail_row_not_a_crash`）。返回 `Some` = 这一组已经
/// 有结论了，别再往下探容器。
fn edge_status_verdict(
    title: &'static str,
    status: &EdgeStatus,
    extra: &mut Map<String, Value>,
) -> Option<CheckResult> {
    extra.insert("edge_version".into(), json!(status.version));
    extra.insert(
        "edge_contract_version".into(),
        json!(status.contract_version),
    );
    extra.insert("sandbox_ok".into(), json!(status.sandbox_ok));
    if status.contract_version != CONTRACT_VERSION {
        return Some(
            fail(
                "sandbox",
                title,
                format!(
                    "两边契约版本不一致：core {CONTRACT_VERSION} vs edge {}",
                    status.contract_version
                ),
            )
            .with_fix("两个进程要一起升：重新 `cargo build` + `go build` 之后再起")
            .with_extra(extra.clone()),
        );
    }
    if !status.sandbox_ok {
        return Some(
            fail(
                "sandbox",
                title,
                "edge 连得上，但它报 docker daemon 不可达（EdgeStatus.sandbox_ok=false）",
            )
            .with_fix("启动 Docker Desktop（或 `colima start`），`docker info` 能出东西再重跑；容器化部署要把 /var/run/docker.sock 挂给 edge")
            .with_extra(extra.clone()),
        );
    }
    None
}

/// 拿到 [`SandboxPort`] 之后的那一段：起容器 → 跑四个 import → **一定收掉**。
///
/// 吃 `&dyn SandboxPort` 而不是自己 `EdgeClient::connect`：这一段是第 6 组最值钱的
/// 部分（收尾顺序、探针炸掉、缺包、镜像不在），而真造这四种现场要一台能按需坏掉的
/// docker。抽出来之后，测试拿一个自写的假 `SandboxPort` 就能把四条判据分支都走一遍。
async fn probe_sandbox(
    sandbox: &dyn SandboxPort,
    cfg: &AiteConfig,
    mut extra: Map<String, Value>,
) -> CheckResult {
    let title = "沙箱可用";
    let task_id = format!("preflight-{}", short_nonce());
    extra.insert("task_id".into(), json!(task_id));
    let mut spec: SandboxSpec = sandbox_spec_of(cfg);
    spec.network = SandboxNetwork::None;

    let started = Instant::now();
    let sandbox_id = match sandbox.acquire(&task_id, &spec).await {
        Ok(id) => id,
        Err(e) => {
            return fail(
                "sandbox",
                title,
                format!("起不了容器：{}", tail(&e.to_string(), 300)),
            )
            .with_fix(format!(
                "docker build -t {} docker/sandbox 重建镜像；镜像里必须有 coreutils 的 timeout，且 /work 可写",
                cfg.sandbox.image
            ))
            .with_extra(extra);
        }
    };

    let req = ExecRequest::python(
        SANDBOX_PROBE_CODE,
        cfg.sandbox.exec_timeout_sec.min(SANDBOX_PROBE_TIMEOUT_SEC),
    );
    let exec = sandbox.exec(&sandbox_id, &req).await;
    // 不管探针成不成，容器一定要收掉 —— 自检自己漏容器比它检出来的问题还讨厌。
    let released = sandbox.release(&sandbox_id).await;
    // 只是「edge 还认不认这个 id」，不是「宿主机上还有没有容器」—— 只进 extra，不进判据。
    let gone = matches!(
        sandbox.list_files(&sandbox_id).await,
        Err(e) if e.kind == SandboxErrorKind::NotFound
    );
    extra.insert("released".into(), json!(released.is_ok()));
    extra.insert("sandbox_gone".into(), json!(gone));
    if let Err(e) = &released {
        // 探针先炸的路径上也得看得见「漏了容器」这件事。
        extra.insert("release_error".into(), json!(tail(&e.to_string(), 300)));
    }

    let result = match exec {
        Ok(r) => r,
        Err(e) => {
            return fail(
                "sandbox",
                title,
                format!("容器起来了但探针没跑成：{}", tail(&e.to_string(), 300)),
            )
            .with_fix("看 edge 的日志与 `docker logs`；镜像里的 python 要能跑 `-c` 脚本")
            .with_extra(extra);
        }
    };
    let elapsed = started.elapsed().as_millis() as u64;
    extra.insert("elapsed_ms".into(), json!(elapsed));
    if result.exit_code != 0 || !result.stdout.contains(SANDBOX_PROBE_MARK) {
        let why = if result.stderr.is_empty() {
            &result.stdout
        } else {
            &result.stderr
        };
        return fail(
            "sandbox",
            title,
            format!(
                "容器起来了但四个 import 没跑通（exit={}）：{}",
                result.exit_code,
                tail(why, 300)
            ),
        )
        .with_fix(format!(
            "镜像缺包。重建：docker build -t {} docker/sandbox；镜像内 \
             `python -c \"import pandas, matplotlib, openpyxl, docx\"` 要能过",
            cfg.sandbox.image
        ))
        .with_extra(extra);
    }
    let versions = parse_probe(&result.stdout);
    let shown = versions
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ");
    extra.insert(
        "packages".into(),
        Value::Object(
            versions
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect(),
        ),
    );
    let detail = format!(
        "{} 起容器 + 四个 import 跑通（{elapsed}ms）· {shown}",
        cfg.sandbox.image
    );
    sandbox_release_verdict(title, &task_id, &detail, released, extra)
}

/// 收尾判据：容器收没收掉，**只认 `release()` 自己的回答**。
///
/// 别拿收尾之后那发 `list_files` 当判据（见 [`check_sandbox`] 的说明）：edge 的
/// `Release` 先删记账再删容器，`ContainerRemove` 失败时记账已经没了，探针照样回
/// `not_found`。真按它判，就会「一边把打着 `aite.task` 标签的容器留在宿主机上
/// 占着 CPU / 内存，一边报『容器已收干净』然后退 0」。
fn sandbox_release_verdict(
    title: &'static str,
    task_id: &str,
    detail: &str,
    released: Result<(), SandboxError>,
    extra: Map<String, Value>,
) -> CheckResult {
    match released {
        Ok(()) => ok("sandbox", title, format!("{detail} · 容器已收干净")).with_extra(extra),
        Err(e) => fail(
            "sandbox",
            title,
            format!(
                "{detail} —— 但自检的容器没收掉（release 失败）：{}",
                tail(&e.to_string(), 300)
            ),
        )
        .with_fix(format!(
            "自检的容器可能还在宿主机上占着资源，先手动收掉：\
             docker rm -f $(docker ps -aq --filter label=aite.task={task_id})；\
             再看 edge 的日志为什么 ContainerRemove 失败，并报告这个现象：收尾路径漏了容器"
        ))
        .with_extra(extra),
    }
}

/// RFC 3986 的 unreserved 集合原样保留，其余按字节 `%XX`。
fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn parse_probe(stdout: &str) -> Vec<(String, String)> {
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix(SANDBOX_PROBE_MARK) {
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(rest.trim()) {
                return map
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            k,
                            v.as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| v.to_string()),
                        )
                    })
                    .collect();
            }
            return Vec::new();
        }
    }
    Vec::new()
}

/// 够用的唯一串（容器标签用）。不拉 uuid 依赖 —— 它不在 app 的依赖表里。
fn short_nonce() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:08x}", (nanos as u64) & 0xffff_ffff)
}

// --------------------------------------------------------------------------
// 7 落盘目录可写
// --------------------------------------------------------------------------

/// 判据是「落得下去」而不是「目录已经在」。
///
/// `FileEvidenceWriter` 和 SQLite store 都是自建目录的，所以真正会让起飞炸掉的是
/// 「最近的那层已存在祖先写不了」，不是「data/ 还没建」。
fn check_storage(cfg: &AiteConfig, repo_root: &Path) -> CheckResult {
    let targets = [
        ("storage.sqlite_path", cfg.storage.sqlite_path.clone()),
        ("storage.evidence_dir", cfg.storage.evidence_dir.clone()),
        ("storage.artifacts_dir", cfg.storage.artifacts_dir.clone()),
    ];
    let resolve = |raw: &str| -> PathBuf {
        let p = Path::new(raw);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            repo_root.join(p)
        }
    };
    // 卡住的那层恰好**就是**仓库根时，`strip_prefix` 得到的是空路径，`display()` 出
    // 空串 —— 打出来是「写不下去：data/aite.db（卡在 ）」，总管看不出卡在哪一层。
    // 空串回退成绝对路径：那一行本来就是要拿去 `ls -ld` 的。
    let rel = |p: &Path| -> String {
        match p.strip_prefix(repo_root) {
            Ok(r) if !r.as_os_str().is_empty() => r.display().to_string(),
            _ => p.display().to_string(),
        }
    };

    let mut bad: Vec<String> = Vec::new();
    let mut todo: Vec<String> = Vec::new();
    let mut extra = Map::new();
    for (field, raw) in &targets {
        let full = resolve(raw);
        // sqlite 是文件，两个 dir 是目录：和 `prepare_storage` 一个口径 —— 要建的是
        // 「sqlite 的父目录」和「两个目录本身」。
        let parent = if *field == "storage.sqlite_path" {
            full.parent().unwrap_or(&full).to_path_buf()
        } else {
            full.clone()
        };
        let mut anchor = parent.clone();
        while !anchor.exists() && anchor.parent().is_some_and(|p| p != anchor) {
            anchor = anchor.parent().unwrap_or(&anchor).to_path_buf();
        }
        let writable = anchor.is_dir() && writable_dir(&anchor);
        extra.insert(
            (*field).to_string(),
            json!({
                "path": raw,
                "parent": rel(&parent),
                "existing_ancestor": rel(&anchor),
                "writable": writable,
            }),
        );
        if !writable {
            bad.push(format!("{raw}（卡在 {}）", rel(&anchor)));
        } else if !parent.exists() && !todo.contains(&rel(&parent)) {
            todo.push(rel(&parent));
        }
    }

    let paths = targets
        .iter()
        .map(|(_f, raw)| raw.clone())
        .collect::<Vec<_>>()
        .join(" · ");
    if !bad.is_empty() {
        return fail(
            "storage",
            "落盘目录可写",
            format!("写不下去：{}", bad.join("；")),
        )
        .with_fix("给这几层目录写权限，或把 config 里的 storage.* 指到一个可写的位置")
        .with_extra(extra);
    }
    let pending = if todo.is_empty() {
        String::new()
    } else {
        format!("（{} 待建，起飞时自动 mkdir）", todo.join(" "))
    };
    ok(
        "storage",
        "落盘目录可写",
        format!("3 个路径都落得下去{pending}：{paths}"),
    )
    .with_extra(extra)
}

/// 目录写得进去吗。不用 libc 的 access() —— 真建一个临时条目再删掉，问的就是要问的那件事。
///
/// **副作用是明知故犯的**，记在这儿免得下一个人当 bug 修掉：这一发会在「三个落盘路径
/// 的最近已存在祖先」里真建一个 `.aite-preflight-<nonce>` 目录再删掉。真机上
/// `evidence_dir` / `artifacts_dir` 已经存在时，探针就落在它们里面。
///
/// 留着的理由：`access(2)` 在 macOS 上对 ACL、只读挂载、以及容器里的 bind mount 都判不准，
/// 而 preflight 的全部价值就是「别让总管在飞书群里瞎试」—— 一个判不准的第 7 组比没有
/// 第 7 组更害人。真写一次是唯一诚实的问法，而 preflight 本来就是起飞前跑的。
///
/// 代价（也是残留面）：进程恰好在 `create_dir` 与 `remove_dir` 之间被 kill，会留下一个
/// 空目录。它不进任何判据、不影响起飞，下次 `rm -rf` 掉即可 —— nonce 让它不会撞名，
/// 也就不会把上一次的残留误判成「写得进去」。
fn writable_dir(dir: &Path) -> bool {
    let probe = dir.join(format!(".aite-preflight-{}", short_nonce()));
    match std::fs::create_dir(&probe) {
        Ok(()) => {
            let _ = std::fs::remove_dir(&probe);
            true
        }
        Err(_) => false,
    }
}

// --------------------------------------------------------------------------
// 编排
// --------------------------------------------------------------------------

pub struct Options {
    pub config: Option<String>,
    pub offline: bool,
    pub json: bool,
    pub chat_id: Option<String>,
    pub repo_root: PathBuf,
    pub feishu_domain: String,
}

/// 七组依次跑完再汇总 —— 一项失败绝不阻断后面的。
pub async fn run_checks(
    opts: &Options,
    env: &HashMap<String, String>,
    redactor: &mut Redactor,
) -> Report {
    let (config_path, fell_back) = pick_config(opts.config.as_deref(), &opts.repo_root);
    let shown = config_path
        .strip_prefix(&opts.repo_root)
        .map(|r| r.display().to_string())
        .unwrap_or_else(|_| config_path.display().to_string());

    // 装料必须赶在 `check_config` 之前。配置读不出来时走的是下面那条早退路径，而
    // `redactor.add()` 原本的五个调用点全在第 3/4/5 组里 —— 那条路上一个都到不了，
    // [`Redactor`] 是空的。偏偏 `check_config` 的 FAIL detail 回显的是 serde_yaml 的
    // 错误，**它会把出错的标量原样打出来**：真机上最典型的形态就是用户把 app_secret
    // 粘进了 `config/aite.yaml`，于是自检一边教他「密钥只写环境变量名，不写取值」，
    // 一边把取值打在屏幕上。这里先按契约默认的那几个 `*_env` 兜一遍。
    arm_redactor(redactor, &env_var_names(&AiteConfig::default()), env);

    let mut notes: Vec<Note> = Vec::new();
    let (cfg_result, cfg) = check_config(&config_path, fell_back, &opts.repo_root);
    let mut checks = vec![cfg_result];

    let Some(cfg) = cfg else {
        // 配置都读不出来，剩下六项的判据无从谈起；照样各占一行，说清为什么。
        for (name, title) in [
            ("env", "环境变量齐"),
            ("feishu_token", "飞书凭证有效"),
            ("feishu_identity", "飞书身份对得上"),
            ("model", "模型端点通"),
            ("sandbox", "沙箱可用"),
            ("storage", "落盘目录可写"),
        ] {
            checks.push(skipped(name, title, "第 1 组没过，配置读不出来"));
        }
        return Report {
            checks,
            notes,
            config_path: shown,
            offline: opts.offline,
        };
    };

    // 真 config 可以把 `*_env` 指到默认之外的名字上，上面那遍兜不住，按它再补一遍。
    // 装料与第 2 组跑不跑无关 —— fake 那一档下面把第 2 组 SKIP 掉了，但 `check_config`
    // 的 FAIL detail 照样会回显 yaml 里的标量，脱敏这一道一步都不能省。
    arm_redactor(redactor, &env_var_names(&cfg), env);

    // `platform: fake` 压根不连飞书，第 2/3/4 组的判据对它不适用 —— 跟 `--offline`
    // 一样是「这一项没法查」，只是原因写在配置里而不是命令行上。**它不是在掩盖问题**：
    // 真正拦住起飞的那条（fake 要注入）第 1 组已经 FAIL 了，整份自检退出码 1。
    //
    // 两个原因同时成立时**说 fake**：去掉 `--offline` 重跑，这几组仍然不适用，
    // 说「--offline：不碰网络」会让人以为去掉那个开关就查得了。
    //
    // 代价（如实记在这儿）：第 2 组是**泛化**扫 `*_env` 的，整组 SKIP 连
    // `model.api_key_env` 一起跳过了。fake 配置注定在第 1 组 FAIL、必须改回 feishu 重跑，
    // 那一轮会把四个变量全查一遍，所以只是推迟一轮、不会静默放行。
    let fake_platform = cfg.platform == PlatformChoice::Fake;
    if fake_platform {
        checks.push(skipped("env", "环境变量齐", FAKE_PLATFORM_SKIP));
    } else {
        checks.push(check_env(&cfg, env, opts.offline));
    }

    let feishu_na = if fake_platform {
        Some(FAKE_PLATFORM_SKIP)
    } else if opts.offline {
        Some("--offline：不碰网络")
    } else {
        None
    };
    if let Some(reason) = feishu_na {
        checks.push(skipped("feishu_token", "飞书凭证有效", reason));
        checks.push(skipped("feishu_identity", "飞书身份对得上", reason));
        notes.push(note_passive_listen());
        notes.push(note_history_scope_unverified());
    } else {
        let outcome = check_feishu(
            &cfg,
            env,
            redactor,
            opts.chat_id.as_deref(),
            &opts.feishu_domain,
        )
        .await;
        checks.push(outcome.token);
        checks.push(outcome.identity);
        notes.extend(outcome.notes);
    }

    // 第 5/6 组只认 `--offline`：模型端点与沙箱跟 platform 取值无关（fake 平台照样
    // 要跑真沙箱、`provider=scripted` 那条第 5 组自己会给 WARN）。
    if opts.offline {
        checks.push(skipped("model", "模型端点通", "--offline：不碰网络"));
        checks.push(skipped("sandbox", "沙箱可用", "--offline：不碰 docker"));
    } else {
        checks.push(check_model(&cfg, env, redactor).await);
        checks.push(check_sandbox(&cfg, &opts.repo_root).await);
    }

    checks.push(check_storage(&cfg, &opts.repo_root));
    Report {
        checks,
        notes,
        config_path: shown,
        offline: opts.offline,
    }
}

fn pick_config(explicit: Option<&str>, repo_root: &Path) -> (PathBuf, bool) {
    let resolve = |raw: &str| -> PathBuf {
        let p = Path::new(raw);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            repo_root.join(p)
        }
    };
    if let Some(p) = explicit {
        return (resolve(p), false);
    }
    let real = resolve(DEFAULT_CONFIG_PATH);
    if real.exists() {
        return (real, false);
    }
    (resolve(EXAMPLE_CONFIG_PATH), true)
}

// --------------------------------------------------------------------------
// 渲染
// --------------------------------------------------------------------------

pub fn render_text(report: &Report, redactor: &Redactor) -> String {
    let mut out = String::new();
    let total = report.checks.len();
    let mode = if report.offline {
        "--offline（只跑 1/2/7）"
    } else {
        "完整（七组）"
    };
    out.push_str("Aite 起飞前自检\n");
    out.push_str(&format!(
        "  配置：{}\n",
        redactor.scrub(&report.config_path)
    ));
    out.push_str(&format!("  模式：{mode}\n"));
    out.push_str(&format!(
        "  时间：{}\n\n",
        chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
    ));

    for (i, c) in report.checks.iter().enumerate() {
        out.push_str(&format!(
            "[{}/{}] {:<4} {} {}\n",
            i + 1,
            total,
            c.status.label(),
            pad(c.title, TITLE_WIDTH),
            redactor.scrub(&c.detail)
        ));
        if !c.fix.is_empty() && c.status != Status::Ok {
            out.push_str(&format!(
                "           └ 怎么补：{}\n",
                redactor.scrub(&c.fix)
            ));
        }
        if c.name == NOTES_AFTER && !report.notes.is_empty() {
            for note in &report.notes {
                out.push_str(&format!(
                    "       NOTE {} {}\n",
                    pad("§3.7 待核实", TITLE_WIDTH),
                    redactor.scrub(&note.detail)
                ));
            }
        }
    }

    let count = |s: Status| report.checks.iter().filter(|c| c.status == s).count();
    out.push('\n');
    out.push_str(&"-".repeat(72));
    out.push('\n');
    out.push_str(&format!(
        "汇总：OK {} · WARN {} · FAIL {} · SKIP {}（共 {total} 项，过了 {} 项）\n",
        count(Status::Ok),
        count(Status::Warn),
        count(Status::Fail),
        count(Status::Skip),
        report.passed()
    ));
    let failed: Vec<&str> = report
        .checks
        .iter()
        .filter(|c| c.failed())
        .map(|c| c.title)
        .collect();
    if failed.is_empty() {
        out.push_str("全部没红，可以起飞。");
        if report.offline {
            out.push_str("（--offline 只验了 1/2/7，真机起飞前请全跑一遍）");
        }
        out.push('\n');
    } else {
        out.push_str(&format!(
            "FAIL：{} —— 起飞前把上面的「怎么补」做掉再跑一次。\n",
            failed.join("、")
        ));
    }
    out
}

pub fn render_json(report: &Report, redactor: &Redactor) -> String {
    let payload = json!({
        "ok": report.ok(),
        "offline": report.offline,
        "config_path": redactor.scrub(&report.config_path),
        "passed": report.passed(),
        "total": report.checks.len(),
        "checks": report.checks.iter().map(|c| json!({
            "name": c.name,
            "title": c.title,
            "status": c.status.as_str(),
            "detail": redactor.scrub(&c.detail),
            "fix": redactor.scrub(&c.fix),
            "extra": redactor.scrub_value(&Value::Object(c.extra.clone())),
        })).collect::<Vec<_>>(),
        "notes": report.notes.iter().map(|n| json!({
            "name": n.name,
            "status": n.status,
            "detail": redactor.scrub(&n.detail),
        })).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&payload).unwrap_or_default()
}

// --------------------------------------------------------------------------
// 入口
// --------------------------------------------------------------------------

const USAGE: &str = "\
用法：
  aite preflight [--config PATH] [--offline] [--json] [--chat-id ID]

  --config PATH   配置文件路径（默认 config/aite.yaml，不存在则退到 config/aite.example.yaml）
  --offline       只跑 1 配置 / 2 环境变量 / 7 落盘目录，不碰网络也不碰 docker（CI、没凭证的机器）
  --json          机器可读输出（每项 name / status / detail）
  --chat-id ID    测试群的 chat_id；给了就顺带实测一次群历史，回答 §3.7(b) 那条待核实项

退出码：任一 FAIL → 1，否则 0。任何模式下都不打印密钥取值。";

/// `aite preflight` 的入口：收到的是 `preflight` 之后的全部参数，返回进程退出码。
pub fn run(argv: Vec<String>) -> i32 {
    let mut opts = Options {
        config: None,
        offline: false,
        json: false,
        chat_id: None,
        repo_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        feishu_domain: DEFAULT_DOMAIN.to_string(),
    };
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--config" => {
                i += 1;
                match argv.get(i) {
                    Some(v) => opts.config = Some(v.clone()),
                    None => {
                        eprintln!("--config 后面缺一个路径");
                        return 2;
                    }
                }
            }
            "--chat-id" => {
                i += 1;
                match argv.get(i) {
                    Some(v) => opts.chat_id = Some(v.clone()),
                    None => {
                        eprintln!("--chat-id 后面缺一个 chat_id");
                        return 2;
                    }
                }
            }
            "--offline" => opts.offline = true,
            "--json" => opts.json = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            other => {
                eprintln!("不认识的参数 {other:?}\n{USAGE}");
                return 2;
            }
        }
        i += 1;
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("preflight 起不来：tokio runtime 建不起来：{e}");
            return 2;
        }
    };
    let env = env_snapshot();
    let mut redactor = Redactor::new();
    let report = runtime.block_on(run_checks(&opts, &env, &mut redactor));
    let text = if opts.json {
        render_json(&report, &redactor)
    } else {
        render_text(&report, &redactor)
    };
    print!("{text}");
    if !text.ends_with('\n') {
        println!();
    }
    if report.ok() { 0 } else { 1 }
}

// --------------------------------------------------------------------------
// 单元测试
// --------------------------------------------------------------------------
//
// 这里钉两件**别处钉不住**的事：
//
// 1. **密钥红线**。`tests/cli_smoke.rs` 那条跑的是 `--offline`，而 `redactor.add()`
//    的全部调用点都在第 3/4/5 组里 —— offline 下这几组全 skip，取值根本没机会进报告，
//    「输出里没有取值」在那一档是恒真的（把整个 [`Redactor`] 删掉它都不会红）。
//    所以三层各钉一条：纯函数（scrub / scrub_value / 长度闸）、`add()` 的调用点、
//    渲染前是否真的过了一遍。对应 Python 那边被删掉的 `test_redactor_*` 与
//    `test_secret_values_never_reach_output`。
// 2. **第 6 组的收尾判据**。真造「`ContainerRemove` 失败但 edge 的记账已删」的现场
//    要一台能坏的 docker，这里退一步：判据抽成纯函数 [`sandbox_release_verdict`]，
//    直接把那个组合喂进去。
#[cfg(test)]
mod tests {
    use super::*;
    // 产品代码没显式提到它（`exec()` 的返回类型是推断出来的），只有测试要造探针结果。
    use aite_contracts::ExecResult;

    /// 假取值。真密钥一个字都不会出现在这个文件里。
    const FAKE_KEY: &str = "sk-preflight-unit-fake-key";
    const FAKE_APP_ID: &str = "cli_unit_fake_app_id";
    const FAKE_APP_SECRET: &str = "unit-fake-app-secret";
    const FAKE_OPEN_ID: &str = "ou_unit_fake_open_id";
    const PLACEHOLDER: &str = "的取值已隐去";

    // ---- 脱敏：纯函数 ------------------------------------------------------

    #[test]
    fn scrub_swaps_the_value_for_a_placeholder() {
        let mut r = Redactor::new();
        r.add(FAKE_KEY, "AITE_MODEL_API_KEY");

        let out = r.scrub(&format!("401 invalid api key {FAKE_KEY} from upstream"));

        assert!(!out.contains(FAKE_KEY), "{out}");
        assert!(out.contains(PLACEHOLDER), "{out}");
        assert!(
            out.contains("AITE_MODEL_API_KEY"),
            "占位符要说清抹的是哪一项：{out}"
        );
    }

    /// 对拍 Python 的 `test_redactor_scrubs_nested_extra`：`extra` 是嵌套的，脱敏要递归下去。
    ///
    /// 取值故意带引号和反斜杠 —— 这正是 [`Redactor::scrub_value`] 不走
    /// 「to_string → 字符串替换 → from_str」那条近路的理由：序列化会把它们转义掉，
    /// 字符串替换就对不上，脱敏静默失效。
    #[test]
    fn scrub_value_reaches_into_nested_extra() {
        let secret = "se\"cret\\value";
        let mut r = Redactor::new();
        r.add(secret, "AITE_MODEL_API_KEY");

        let scrubbed = r.scrub_value(&json!({"a": [secret], "b": {"c": format!("x {secret} y")}}));

        let dumped = serde_json::to_string(&scrubbed).unwrap();
        assert!(!dumped.contains(secret), "{dumped}");
        // 转义后的形态也不许在：只查原文的话，`"` 被写成 `\"` 就查不着了。
        let escaped = serde_json::to_string(secret).unwrap();
        assert!(!dumped.contains(escaped.trim_matches('"')), "{dumped}");
        assert!(scrubbed["a"][0].as_str().unwrap().contains(PLACEHOLDER));
        assert!(scrubbed["b"]["c"].as_str().unwrap().contains(PLACEHOLDER));
    }

    /// 对拍 Python 的 `test_redactor_leaves_short_values_alone`：
    /// 短于 [`MIN_REDACT_LEN`] 的值不替换，否则正常输出会被打成马赛克。
    #[test]
    fn scrub_leaves_values_shorter_than_the_floor_alone() {
        let mut short = Redactor::new();
        short.add("ab", "SHORT");
        assert_eq!(short.scrub("about"), "about");

        // 边界：正好 MIN_REDACT_LEN 个字符的要替换，闸门别开偏一位。
        let just_long_enough = "x".repeat(MIN_REDACT_LEN);
        let mut r = Redactor::new();
        r.add(&just_long_enough, "LONG_ENOUGH");
        let out = r.scrub(&format!("v={just_long_enough}"));
        assert!(!out.contains(&just_long_enough), "{out}");
        assert!(out.contains(PLACEHOLDER), "{out}");
    }

    // ---- 脱敏：接线 --------------------------------------------------------

    /// 渲染前真的过了一遍：detail / fix / 嵌套 extra / note / config_path 一处都不能漏。
    /// 把 `render_text` / `render_json` 里任何一处 `redactor.scrub*` 拆掉，这条就红。
    #[test]
    fn every_rendered_field_goes_through_the_redactor() {
        let mut r = Redactor::new();
        r.add(FAKE_KEY, "AITE_MODEL_API_KEY");
        let mut extra = Map::new();
        extra.insert(
            "upstream".into(),
            json!({"msg": [format!("bad key {FAKE_KEY}")]}),
        );
        // name 用 NOTES_AFTER，这样 render_text 也会把 note 那行渲出来。
        let report = Report {
            checks: vec![CheckResult {
                name: NOTES_AFTER,
                title: "飞书身份对得上",
                status: Status::Fail,
                detail: format!("上游把凭证回显进错误消息了：bad token {FAKE_KEY}"),
                fix: format!("换掉 {FAKE_KEY} 这个取值"),
                extra,
            }],
            notes: vec![Note {
                name: "feishu_group_history_scope",
                status: "unverified",
                detail: format!("上游回显：{FAKE_KEY}"),
            }],
            config_path: format!("config/{FAKE_KEY}.yaml"),
            offline: false,
        };

        let text = render_text(&report, &r);
        let raw_json = render_json(&report, &r);

        for out in [&text, &raw_json] {
            assert!(!out.contains(FAKE_KEY), "取值漏进渲染结果了：{out}");
            // 反过来钉一条：确实是被抹掉的，而不是这几段文本压根没走到输出。
            assert!(out.contains(PLACEHOLDER), "{out}");
        }
        let parsed: Value = serde_json::from_str(&raw_json).unwrap();
        assert!(
            parsed["checks"][0]["extra"]["upstream"]["msg"][0]
                .as_str()
                .unwrap()
                .contains(PLACEHOLDER),
            "嵌套 extra 没洗：{raw_json}"
        );
    }

    /// 第 5 组把模型密钥交给了 Redactor —— 删掉 `check_model` 里那句
    /// `redactor.add(v, &mcfg.api_key_env)`，这条就红。
    ///
    /// 走「配置不全」那条早退路径（默认 config 的 base_url / model 都是空的）：
    /// 不碰网络，也不需要端点通。
    #[tokio::test]
    async fn check_model_hands_the_api_key_to_the_redactor() {
        let cfg = AiteConfig::default();
        let env = HashMap::from([(cfg.model.api_key_env.clone(), FAKE_KEY.to_string())]);
        let mut r = Redactor::new();

        let result = check_model(&cfg, &env, &mut r).await;

        assert_eq!(result.status, Status::Fail, "{}", result.detail);
        let echoed = r.scrub(&format!("401 invalid api key {FAKE_KEY}"));
        assert!(
            !echoed.contains(FAKE_KEY),
            "第 5 组没把模型密钥交给 Redactor：{echoed}"
        );
    }

    /// 第 3/4 组把飞书三项都交给了 Redactor —— 删掉 `check_feishu` 里那三句 `add`
    /// 的任何一句，这条就红。
    ///
    /// `domain` 故意给个不是 URL 的串：reqwest 在 builder 阶段就报错，一个包都不发，
    /// 也不会等超时。
    #[tokio::test]
    async fn check_feishu_hands_all_three_env_values_to_the_redactor() {
        let cfg = AiteConfig::default();
        let env = HashMap::from([
            (cfg.feishu.app_id_env.clone(), FAKE_APP_ID.to_string()),
            (
                cfg.feishu.app_secret_env.clone(),
                FAKE_APP_SECRET.to_string(),
            ),
            (cfg.feishu.bot_open_id_env.clone(), FAKE_OPEN_ID.to_string()),
        ]);
        let mut r = Redactor::new();

        let outcome = check_feishu(&cfg, &env, &mut r, None, "not-a-url").await;

        assert_eq!(
            outcome.token.status,
            Status::Fail,
            "{}",
            outcome.token.detail
        );
        for (value, what) in [
            (FAKE_APP_ID, "app_id"),
            (FAKE_APP_SECRET, "app_secret"),
            (FAKE_OPEN_ID, "bot_open_id"),
        ] {
            let echoed = r.scrub(&format!("上游回显了 {value}"));
            assert!(!echoed.contains(value), "{what} 没交给 Redactor：{echoed}");
        }
    }

    // ---- 第 6 组：收尾判据 -------------------------------------------------

    fn verdict(released: Result<(), SandboxError>, gone: bool) -> CheckResult {
        let mut extra = Map::new();
        extra.insert("released".into(), json!(released.is_ok()));
        extra.insert("sandbox_gone".into(), json!(gone));
        sandbox_release_verdict(
            "沙箱可用",
            "preflight-deadbeef",
            "aite-sandbox:p0 起容器 + 四个 import 跑通（120ms）",
            released,
            extra,
        )
    }

    /// 真现场：edge 的 `Release` 先删记账再 `ContainerRemove`，所以后者失败时记账
    /// 已经没了 —— 紧跟着那发 `list_files` 照样回 NotFound（`gone=true`）。判据要是
    /// 看 `gone`，这一组就会一边把容器漏在宿主机上，一边报「容器已收干净」并退 0。
    #[test]
    fn release_failure_fails_the_row_even_though_edge_already_forgot_the_id() {
        let boom = SandboxError::new(
            SandboxErrorKind::Internal,
            "释放沙箱失败（0a1b2c3d）：Error response from daemon: removal already in progress",
        );

        let r = verdict(Err(boom), true);

        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(!r.detail.contains("容器已收干净"), "{}", r.detail);
        assert!(
            r.detail.contains("释放沙箱失败"),
            "错误原文要带出来：{}",
            r.detail
        );
        assert!(
            r.fix.contains(
                "docker rm -f $(docker ps -aq --filter label=aite.task=preflight-deadbeef)"
            ),
            "「怎么补」要能直接粘：{}",
            r.fix
        );
    }

    /// 反过来：`release()` 说收掉了就是收掉了，`list_files` 那发只搭车进 extra。
    #[test]
    fn list_files_probe_only_rides_along_as_extra() {
        let r = verdict(Ok(()), false);

        assert_eq!(r.status, Status::Ok, "{}", r.detail);
        assert!(r.detail.contains("容器已收干净"), "{}", r.detail);
        assert_eq!(r.extra["released"], json!(true));
        assert_eq!(r.extra["sandbox_gone"], json!(false));
    }

    // =======================================================================
    // 以下为 V2 接管 Python `tests/scripts/test_preflight.py`（564 行 / 25 条）的部分。
    //
    // 分工：私有函数（`check_*` / `probe_sandbox` / `arm_redactor` / `error_chain`）钉在
    // 这里；公开面（`run_checks` / `render_*` / 七行齐不齐 / 脱敏端到端）钉在
    // `tests/preflight_e2e.rs`，那边有一台按 path 路由的 HTTP 假服务。
    // =======================================================================

    /// 一条能自己嵌套的假错误，用来造 `source()` 链。
    #[derive(Debug)]
    struct Layer(String, Option<Box<Layer>>);

    impl std::fmt::Display for Layer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl std::error::Error for Layer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1
                .as_deref()
                .map(|inner| inner as &(dyn std::error::Error + 'static))
        }
    }

    fn layer(msg: &str, inner: Option<Layer>) -> Layer {
        Layer(msg.to_string(), inner.map(Box::new))
    }

    /// 链要一路走到底 —— 根因在最后一层，而 `Display` 只给得出第一层。
    ///
    /// 这就是 §6.2 那条的根：`reqwest::Error` 的 Display 只写 kind + url，于是「代理端口
    /// 关着」「代理主机名解析不了」「TLS 被中间设备换掉」三种病打出来一模一样。
    #[test]
    fn error_chain_walks_all_the_way_down() {
        let e = layer(
            "error sending request for url (https://open.feishu.cn/…)",
            Some(layer(
                "client error (Connect)",
                Some(layer(
                    "tcp connect error: Connection refused (os error 61)",
                    None,
                )),
            )),
        );

        let out = error_chain(&e);

        assert!(out.starts_with("error sending request"), "{out}");
        assert!(
            out.contains("Connection refused (os error 61)"),
            "根因没拼上，这条链等于白走：{out}"
        );
        // 自检行还要过 tail()，截的是尾巴 —— 根因必须活到截断之后。
        assert!(
            tail(&out, 60).contains("Connection refused"),
            "根因被 tail 截掉了，那拼链就白拼：{}",
            tail(&out, 60)
        );
    }

    /// 上游常把下一层的 `Display` 原样嵌进自己的消息里（hyper / rustls 都这么干），
    /// 拼两遍只会更难读。
    #[test]
    fn error_chain_skips_a_layer_the_parent_already_quoted() {
        let e = layer(
            "dns error: failed to lookup address information",
            Some(layer("failed to lookup address information", None)),
        );

        let out = error_chain(&e);

        assert_eq!(
            out.matches("failed to lookup address information").count(),
            1,
            "{out}"
        );
        assert!(!out.contains(" ← "), "重复的一层不该拼进来：{out}");
    }

    // ---- 脱敏：字符闸（§6.5）与装料（§6.3 / §6.4）--------------------------

    /// [`MIN_REDACT_LEN`] 数的是**字符**不是字节。
    ///
    /// 改之前 `"配置".len()` 是 6，两个汉字就过闸 —— 于是自检输出里凡出现「配置」两个字
    /// 都被替换掉。真跑出来的样子（`AITE_MODEL_API_KEY` 恰好设成了「配置」两个字）：
    /// 「配置不全，缺：model.base_url / model.model」变成
    /// 「«AITE_MODEL_API_KEY 的取值已隐去»不全，缺：…」，第 6 组的「怎么补」里
    /// 「aite-edge --config <配置文件>」也被打成同样的马赛克。preflight 最该说人话的
    /// 时刻，反被自己的脱敏闸打烂。
    ///
    /// [`scrub_leaves_values_shorter_than_the_floor_alone`] 那条用的是
    /// `"x".repeat(MIN_REDACT_LEN)`，字节与字符恰好一致，钉不住这一条。
    #[test]
    fn the_floor_counts_characters_not_bytes() {
        let mut r = Redactor::new();
        r.add("配置", "AITE_MODEL_API_KEY");

        assert_eq!(
            r.scrub("配置不全，缺：model.base_url / model.model"),
            "配置不全，缺：model.base_url / model.model"
        );

        // 边界的另一头：正好 MIN_REDACT_LEN 个**字符**的非 ASCII 取值照样要抹掉。
        let four_chars = "配置密钥";
        assert_eq!(four_chars.chars().count(), MIN_REDACT_LEN);
        assert!(
            four_chars.len() > MIN_REDACT_LEN,
            "这条边界得靠多字节才有意义"
        );
        let mut long = Redactor::new();
        long.add(four_chars, "AITE_MODEL_API_KEY");

        let out = long.scrub(&format!("v={four_chars}"));

        assert!(!out.contains(four_chars), "{out}");
        assert!(out.contains(PLACEHOLDER), "{out}");
    }

    /// 对拍 Python 的 `test_env_var_names_covers_every_env_field`，但判据换了个写法。
    ///
    /// Python 那条是快照（「等于那四个名字」）。照搬的话，契约加第 5 个 `*_env` 的那天
    /// 它会红 —— 可红的是**测试过期**，不是**脱敏漏了**，读的人还得先分辨是哪一种。
    /// 这里钉的是那层恒等关系：[`env_var_names`] 报出来的每一项，只要环境里有取值，
    /// [`Redactor`] 就必须认得它。名字表里多出来的那一项就是「契约以后加的第 5 个
    /// `*_env`」—— 把 [`arm_redactor`] 改回写死 `cfg.feishu.app_id_env` 那四个字段，
    /// 这一条立刻红，而契约真加第 5 个的那天它自动跟着覆盖，不用改一个字。
    #[test]
    fn arm_redactor_covers_every_name_the_contract_reports() {
        let cfg = AiteConfig::default();
        let mut names = env_var_names(&cfg);
        assert!(names.len() >= 4, "契约至少点名四个 *_env：{names:?}");
        names.push((
            "future.some_new_token_env".to_string(),
            "AITE_FUTURE_FAKE_TOKEN".to_string(),
        ));

        let env: HashMap<String, String> = names
            .iter()
            .enumerate()
            .map(|(i, (_f, var))| (var.clone(), format!("fake-value-{i}-for-{var}")))
            .collect();
        let mut r = Redactor::new();
        arm_redactor(&mut r, &names, &env);

        for (field, var) in &names {
            let value = &env[var];
            let out = r.scrub(&format!("上游回显了 {value}"));
            assert!(
                !out.contains(value.as_str()),
                "{field}（{var}）的取值没进脱敏表：{out}"
            );
            assert!(out.contains(PLACEHOLDER), "{out}");
        }
    }

    /// 环境里没有的变量不占位 —— 空取值进了表会把 [`Redactor::scrub`] 变成噪音源。
    #[test]
    fn arm_redactor_ignores_names_with_no_value_in_the_environment() {
        let names = vec![(
            "model.api_key_env".to_string(),
            "AITE_UNSET_FAKE".to_string(),
        )];
        let mut r = Redactor::new();

        arm_redactor(&mut r, &names, &HashMap::new());

        assert_eq!(r.scrub("原样不动的一句话"), "原样不动的一句话");
    }

    // ---- 第 1 / 2 组 -------------------------------------------------------

    /// 对拍 Python 的 `test_bad_config_fails_but_still_reports_seven_rows` 的前一半：
    /// yaml 读不懂就是 FAIL，且不许交出一个半成品 config。
    #[test]
    fn check_config_fails_on_a_yaml_it_cannot_read() {
        let root = tempfile::tempdir().expect("tempdir");
        let bad = root.path().join("bad.yaml");
        std::fs::write(&bad, "platform: 不存在的平台\n").expect("write");

        let (r, cfg) = check_config(&bad, false, root.path());

        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(cfg.is_none(), "读不出来就不该交出 config");
        assert!(
            r.fix.contains("密钥只写环境变量名，不写取值"),
            "「怎么补」要把红线说出来：{}",
            r.fix
        );
    }

    /// [`DEFAULT_SYSTEM_PROMPT_PATH`] 必须和契约默认值对得上。
    ///
    /// 这个常量只在「怎么补」里露面 —— 它就是那句「该改成什么」。契约哪天把 prompt 挪了窝
    /// 而这里没跟上，自检会一脸笃定地把人指到一个不存在的路径上，**而且没有任何别的测试
    /// 会红**（第 1 组照样 FAIL、照样给 fix，只是那句 fix 是错的）。所以拿契约当唯一真值源
    /// 钉一次。`config/aite.example.yaml` 里那一行由 `check_config_points_at_the_example`
    /// 一起验，三处同源。
    #[test]
    fn prompt_default_matches_the_contract() {
        assert_eq!(
            DEFAULT_SYSTEM_PROMPT_PATH,
            AiteConfig::default().worker.system_prompt_path,
            "「怎么补」里那句「该改成什么」和契约默认值分家了"
        );
    }

    /// 样例配置里那一行也得是同一个值 —— 「怎么补」把人指向样例，指错了就白指。
    ///
    /// 读的是仓库里的真文件（`CARGO_MANIFEST_DIR` 往上三层）。有人动了样例里的
    /// `worker.system_prompt_path` 而没动这边，这条会红。
    #[test]
    fn check_config_points_at_the_example() {
        let example = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join(EXAMPLE_CONFIG_PATH);
        let text = std::fs::read_to_string(&example)
            .unwrap_or_else(|e| panic!("读不到 {}：{e}", example.display()));
        assert!(
            text.contains(&format!("system_prompt_path: {DEFAULT_SYSTEM_PROMPT_PATH}")),
            "{EXAMPLE_CONFIG_PATH} 里的 worker.system_prompt_path 和「怎么补」指的不是同一个值"
        );
    }

    /// 第 1 组的新判据，纯函数这一层：prompt 指不到就是 FAIL，**但 config 要照常交出去**。
    ///
    /// 最后那半句才是容易写错的地方：yaml 读不懂时不交 config（后面六组没判据可谈），
    /// 而这里配置本身是好的 —— 交不出去的话后面六组会全变成「第 1 组没过，配置读不出来」，
    /// 「一项失败不阻断后面的」当场破功。端到端那一面钉在 `tests/preflight_e2e.rs`。
    #[test]
    fn check_config_fails_but_still_hands_over_the_config_when_the_prompt_is_missing() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("aite.yaml");
        std::fs::write(
            &path,
            "worker:\n  system_prompt_path: aite/worker/prompts/platform.md\n",
        )
        .expect("write");

        let (r, cfg) = check_config(&path, false, root.path());

        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(cfg.is_some(), "配置本身是好的，后面六组还要用它");
        assert!(
            r.detail.contains("worker.system_prompt_path"),
            "{}",
            r.detail
        );
        assert!(
            r.fix.contains(DEFAULT_SYSTEM_PROMPT_PATH),
            "「怎么补」里没有正确路径：{}",
            r.fix
        );
        assert_eq!(r.extra["system_prompt_ok"], json!(false));
    }

    /// 相对路径按 `repo_root` 解析 —— 这条钉的是「两边口径一致」的那个接缝。
    ///
    /// `repo_root` 在真跑时就是进程 cwd（[`run`] 里 `current_dir()`），而
    /// `require_system_prompt` 也是相对 cwd 读的。这里改成按别的什么解析（比如相对
    /// 配置文件所在目录），preflight 就会在总管那台机器上说「行」而 `aite run` 说「不行」，
    /// 或者反过来 —— 正是这一轨要根治的那个病。
    #[test]
    fn check_config_resolves_the_prompt_under_the_repo_root() {
        let root = tempfile::tempdir().expect("tempdir");
        // 配置文件搁在子目录里：相对配置文件解析的话，下面这份 prompt 就找不到了。
        let sub = root.path().join("etc");
        std::fs::create_dir_all(&sub).expect("mkdir");
        let path = sub.join("aite.yaml");
        std::fs::write(
            &path,
            "worker:\n  system_prompt_path: prompts/platform.md\n",
        )
        .expect("write");
        std::fs::create_dir_all(root.path().join("prompts")).expect("mkdir");
        std::fs::write(root.path().join("prompts/platform.md"), "# 假 prompt\n").expect("write");

        let (r, _cfg) = check_config(&path, false, root.path());

        assert_eq!(r.status, Status::Ok, "{} / {}", r.detail, r.fix);
        assert_eq!(
            r.extra["system_prompt_resolved"],
            json!(
                root.path()
                    .join("prompts/platform.md")
                    .display()
                    .to_string()
            )
        );
    }

    /// 「怎么补」要说破那条真实病史 —— 只有认出旧 Python 树时才说，别处不说。
    ///
    /// 两半都得钉：说破的那一半是给总管看的（他撞上的就是这一种，不说破他只看见一个
    /// 路径不存在）；不说的那一半是防噪音 —— 路径明明不是那个还硬贴一段 2026-09-12 的
    /// 病史，只会让人往错的方向查。
    #[test]
    fn the_fix_only_blames_the_deleted_python_tree_when_it_is_actually_to_blame() {
        let stale = system_prompt_fix(
            "aite/worker/prompts/platform.md",
            Path::new("/repo/aite/worker/prompts/platform.md"),
        );
        assert!(stale.contains("2026-09-12"), "{stale}");
        assert!(stale.contains(DELETED_PYTHON_PROMPT_DIR), "{stale}");

        let typo = system_prompt_fix(
            "prompts/platfrom.md",
            Path::new("/repo/prompts/platfrom.md"),
        );
        assert!(
            !typo.contains("2026-09-12"),
            "路径不是旧 Python 树，不该往那儿带：{typo}"
        );
        assert!(typo.contains("工作目录"), "别的病因也得给一句：{typo}");
        // 两种情况都要说清「该改成什么」，那是「怎么补」的本分。
        for fix in [&stale, &typo] {
            assert!(fix.contains(DEFAULT_SYSTEM_PROMPT_PATH), "{fix}");
            assert!(fix.contains(EXAMPLE_CONFIG_PATH), "{fix}");
        }
    }

    /// [`REAL_PLATFORM`] / [`REAL_MODEL_PROVIDER`] 必须和契约默认值对得上。
    ///
    /// 同 [`prompt_default_matches_the_contract`] 那条的道理：这两个常量只在「怎么补」里
    /// 露面，指错了别的测试一条都不会红 —— 第 1 组照样 FAIL、照样给 fix，只是那句
    /// 「该改成什么」把人指到一个同样起不来的取值上。样例配置里那两行一起验，三处同源。
    #[test]
    fn injection_fix_matches_the_contract_defaults() {
        let def = AiteConfig::default();
        assert_eq!(
            REAL_PLATFORM,
            def.platform.as_str(),
            "「怎么补」里那句「platform 该改成什么」和契约默认值分家了"
        );
        assert_eq!(
            REAL_MODEL_PROVIDER,
            def.model.provider.as_str(),
            "「怎么补」里那句「model.provider 该改成什么」和契约默认值分家了"
        );

        let example = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join(EXAMPLE_CONFIG_PATH);
        let text = std::fs::read_to_string(&example)
            .unwrap_or_else(|e| panic!("读不到 {}：{e}", example.display()));
        assert!(
            text.contains(&format!("platform: {REAL_PLATFORM}")),
            "{EXAMPLE_CONFIG_PATH} 里的 platform 和「怎么补」指的不是同一个值"
        );
        assert!(
            text.contains(&format!("provider: {REAL_MODEL_PROVIDER}")),
            "{EXAMPLE_CONFIG_PATH} 里的 model.provider 和「怎么补」指的不是同一个值"
        );
    }

    /// 能自己起飞的配置一个字都不碰 —— 判据不许误伤 `feishu` + `openai_compat`。
    #[test]
    fn injection_fault_lets_a_config_that_can_fly_through() {
        assert!(injection_fault(&AiteConfig::default()).is_none());
    }

    /// 模型那条照抄 `build_model` 的**不等式**（`provider != OpenaiCompat`），
    /// 不是写死 `== Scripted`。
    ///
    /// 契约现在只有两个 provider，所以这条钉不出「第三个 provider 也该被拦」；能钉的是
    /// **唯一放行的那个是哪个** —— 判据要是写成 `== Scripted`，哪天加了第三个取值，
    /// preflight 会放它起飞而 `build_model` 照样拒绝，两边当场分家。真出现第三个取值时，
    /// `build_app_really_refuses_what_the_first_check_refuses`（e2e）会替这条把关。
    #[test]
    fn injection_fault_only_lets_the_one_provider_through() {
        for p in [ModelProvider::OpenaiCompat, ModelProvider::Scripted] {
            let mut cfg = AiteConfig::default();
            cfg.model.provider = p;
            let got = injection_fault(&cfg);
            assert_eq!(
                got.is_none(),
                p == ModelProvider::OpenaiCompat,
                "provider={p} 的放行判断反了"
            );
        }
    }

    /// 第 1 组的注入判据，纯函数这一层：FAIL，**但 config 要照常交出去**。
    ///
    /// 后半句是 X1 点名的那条口径（prompt 那边同款）：交不出去的话后面六组会全变成
    /// 「第 1 组没过，配置读不出来」，「一项失败不阻断后面的」当场破功。
    #[test]
    fn check_config_fails_but_still_hands_over_the_config_when_the_platform_is_fake() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("aite.yaml");
        std::fs::write(&path, "platform: fake\n").expect("write");
        // prompt 那条判据别来插一脚：把契约默认那个路径真建出来。
        let prompt = root.path().join(DEFAULT_SYSTEM_PROMPT_PATH);
        std::fs::create_dir_all(prompt.parent().expect("有父目录")).expect("mkdir");
        std::fs::write(&prompt, "# 假 prompt\n").expect("write");

        let (r, cfg) = check_config(&path, false, root.path());

        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(cfg.is_some(), "配置本身是好的，后面六组还要用它");
        assert!(r.detail.contains("platform=fake"), "{}", r.detail);
        assert!(r.fix.contains(REAL_PLATFORM), "{}", r.fix);
        assert_eq!(r.extra["needs_injection"], json!(true));
        // prompt 那条没被误伤。
        assert_eq!(r.extra["system_prompt_ok"], json!(true));
    }

    /// 两件事都犯的配置**一次报齐** —— 不是撞上 prompt 那条就早退。
    ///
    /// 早退的话，人补完 prompt 重跑才发现还有 `platform: fake` 挡着，白跑一轮。
    /// 口径照第 5 组那句「缺什么一次报齐，别让人补完 base_url 重跑一遍才发现还缺 key」。
    #[test]
    fn check_config_reports_the_prompt_and_the_injection_in_one_go() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("aite.yaml");
        std::fs::write(
            &path,
            "platform: fake\nmodel:\n  provider: scripted\nworker:\n  \
             system_prompt_path: aite/worker/prompts/platform.md\n",
        )
        .expect("write");

        let (r, cfg) = check_config(&path, false, root.path());

        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(cfg.is_some());
        for keyword in [
            "worker.system_prompt_path",
            "platform=fake",
            "model.provider=scripted",
        ] {
            assert!(
                r.detail.contains(keyword),
                "三件事没报齐，缺 {keyword}：{}",
                r.detail
            );
        }
        // 「怎么补」也要三件齐 —— 报齐了却只教一件，等于没报齐。
        for keyword in [
            DEFAULT_SYSTEM_PROMPT_PATH,
            REAL_PLATFORM,
            REAL_MODEL_PROVIDER,
        ] {
            assert!(r.fix.contains(keyword), "「怎么补」缺 {keyword}：{}", r.fix);
        }
    }

    /// 对拍 Python 的 `test_missing_env_var_fails_and_names_it`：非 offline 下缺变量就是
    /// FAIL，而且要**点名**缺的那一个 —— 点名这件事本身就是判据的一半。
    #[test]
    fn check_env_fails_and_names_the_missing_var() {
        let cfg = AiteConfig::default();
        let names = env_var_names(&cfg);
        let env: HashMap<String, String> = names
            .iter()
            .filter(|(_f, var)| var != &cfg.feishu.bot_open_id_env)
            .map(|(_f, var)| (var.clone(), format!("fake-value-for-{var}")))
            .collect();

        let r = check_env(&cfg, &env, false);

        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(
            r.detail.contains(&format!("1/{} 个未设置", names.len())),
            "{}",
            r.detail
        );
        assert!(
            r.detail.contains(&cfg.feishu.bot_open_id_env),
            "{}",
            r.detail
        );
        assert_eq!(r.extra["missing"], json!([cfg.feishu.bot_open_id_env]));
        assert!(
            r.fix.starts_with("export "),
            "「怎么补」要能直接粘：{}",
            r.fix
        );
        // 红线：第 2 组只报「在不在」，一个取值都不许出现。
        for value in env.values() {
            assert!(!r.detail.contains(value), "第 2 组打出了取值：{}", r.detail);
            assert!(!r.fix.contains(value), "第 2 组打出了取值：{}", r.fix);
        }
    }

    /// 对拍 Python 的 `test_offline_with_no_credentials_still_exits_zero`：
    /// CI 和没凭证的机器上要能跑 —— 缺变量降成 WARN，但一个名字都不少报。
    #[test]
    fn check_env_downgrades_to_warn_offline_but_names_them_all() {
        let cfg = AiteConfig::default();

        let r = check_env(&cfg, &HashMap::new(), true);

        assert_eq!(r.status, Status::Warn, "{}", r.detail);
        assert!(!r.failed(), "WARN 不该拦起飞");
        for (_field, var) in env_var_names(&cfg) {
            assert!(r.detail.contains(&var), "少报了 {var}：{}", r.detail);
        }
        assert_eq!(r.extra["offline_downgraded"], json!(true));
    }

    // ---- 第 3 / 4 组：前置未满足 -------------------------------------------

    /// 对拍 Python 的 `test_missing_feishu_creds_block_checks_three_and_four`：
    /// 本机（没凭证）跑出来的就是这个分支 —— 两组都 FAIL，且说清是前置没满足。
    ///
    /// 「一个包都不发」这件事是这么钉住的：`blocked()` 早退在 `reqwest::Client::builder()`
    /// **之前**，所以拿到的必须是「前置未满足」而不是任何网络错误。`domain` 给一个
    /// 真发就会连接被拒的地址，真发出去了 detail 会变成另一句话，这条就红。
    #[tokio::test]
    async fn check_feishu_blocks_three_and_four_without_sending_anything() {
        let cfg = AiteConfig::default();
        let mut r = Redactor::new();

        let outcome = check_feishu(&cfg, &HashMap::new(), &mut r, None, "http://127.0.0.1:1").await;

        for c in [&outcome.token, &outcome.identity] {
            assert_eq!(c.status, Status::Fail, "{}", c.detail);
            assert!(c.detail.contains("前置未满足"), "{}", c.detail);
            assert!(c.detail.contains(&cfg.feishu.app_id_env), "{}", c.detail);
            assert!(
                c.detail.contains(&cfg.feishu.app_secret_env),
                "{}",
                c.detail
            );
        }
        assert_eq!(outcome.notes.len(), 2, "§3.7 那两条提示行照样要在");
    }

    // ---- 第 6 组：假沙箱 ---------------------------------------------------

    /// 自写的假 [`SandboxPort`]：造得出「镜像不在」「探针炸了」「缺包」「release 失败」
    /// 四种现场，并**按调用顺序记账** —— 收尾的先后顺序是这一组最值钱的判据。
    ///
    /// 为什么不用 `aite-testing` 的 `FakeSandbox`：它没有「让 release 失败」的开关，
    /// 而 `core/crates/testing/**` 不在本轨的可写面上。
    struct StubSandbox {
        acquire_err: Option<SandboxError>,
        exec_result: Result<ExecResult, SandboxError>,
        release_err: Option<SandboxError>,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl StubSandbox {
        /// 一切正常：容器起得来、四个 import 跑得通、收得掉。
        fn green() -> Self {
            Self {
                acquire_err: None,
                exec_result: Ok(probe_ok()),
                release_err: None,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn exec(mut self, r: Result<ExecResult, SandboxError>) -> Self {
            self.exec_result = r;
            self
        }
        fn acquire_fails(mut self, e: SandboxError) -> Self {
            self.acquire_err = Some(e);
            self
        }
        fn release_fails(mut self, e: SandboxError) -> Self {
            self.release_err = Some(e);
            self
        }
        fn log(&self, what: &str) {
            self.calls.lock().expect("calls").push(what.to_string());
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("calls").clone()
        }
        fn at(&self, what: &str) -> Option<usize> {
            self.calls().iter().position(|c| c == what)
        }
    }

    /// 探针跑通时容器回的那一行。
    fn probe_ok() -> ExecResult {
        exec_result(
            0,
            &format!(
                r#"{SANDBOX_PROBE_MARK} {{"pandas":"2.2.3","matplotlib":"3.9.2","openpyxl":"3.1.5","docx":"1.1.2"}}"#
            ),
            "",
        )
    }

    fn exec_result(exit_code: i32, stdout: &str, stderr: &str) -> ExecResult {
        ExecResult {
            exit_code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            duration_ms: 120,
            truncated: false,
            files_out: Vec::new(),
        }
    }

    #[async_trait::async_trait]
    impl SandboxPort for StubSandbox {
        async fn acquire(
            &self,
            task_id: &str,
            _spec: &SandboxSpec,
        ) -> Result<String, SandboxError> {
            self.log("acquire");
            match &self.acquire_err {
                Some(e) => Err(e.clone()),
                None => Ok(format!("sb-for-{task_id}")),
            }
        }
        async fn exec(&self, _id: &str, _req: &ExecRequest) -> Result<ExecResult, SandboxError> {
            self.log("exec");
            self.exec_result.clone()
        }
        async fn list_files(&self, _id: &str) -> Result<Vec<String>, SandboxError> {
            self.log("list_files");
            // 真 edge 的 `Release` 先删记账再 `ContainerRemove`，所以收尾之后这一发回的
            // 就是 NotFound —— 哪怕容器还留在宿主机上。判据不认它，只认 release 自己。
            Err(SandboxError::new(SandboxErrorKind::NotFound, "沙箱不存在"))
        }
        async fn release(&self, _id: &str) -> Result<(), SandboxError> {
            self.log("release");
            match &self.release_err {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }
        // 下面四个第 6 组根本不碰。碰了就是 `probe_sandbox` 变了形状，当场炸出来。
        async fn put_file(&self, _: &str, _: &str, _: &[u8]) -> Result<(), SandboxError> {
            panic!("第 6 组不该碰 put_file")
        }
        async fn get_file(&self, _: &str, _: &str) -> Result<Vec<u8>, SandboxError> {
            panic!("第 6 组不该碰 get_file")
        }
        async fn touch(&self, _: &str) -> Result<(), SandboxError> {
            panic!("第 6 组不该碰 touch")
        }
        async fn reap_idle(&self, _: u32) -> Result<Vec<String>, SandboxError> {
            panic!("第 6 组不该碰 reap_idle")
        }
    }

    /// 一份 storage 指到临时目录的 config。
    ///
    /// 整轨硬约束：`core/crates/app/tests/` 一个字节都不许写进仓库的 `data/`
    /// （`tests/cold_start_to_delivery.rs:220`、`tests/evidence_on_disk.rs:340`）。
    /// 第 7 组的可写探测是**真建一个目录再删掉**，所以 `storage.*` 一律指到 tempfile。
    fn cfg_in(root: &Path) -> AiteConfig {
        let mut cfg = AiteConfig::default();
        cfg.storage.sqlite_path = root.join("data/aite.db").display().to_string();
        cfg.storage.evidence_dir = root.join("data/evidence").display().to_string();
        cfg.storage.artifacts_dir = root.join("data/artifacts").display().to_string();
        cfg
    }

    /// 对拍 Python 的 `test_sandbox_releases_container_on_success` +
    /// `test_json_carries_model_and_sandbox_evidence` 的沙箱那一半。
    #[tokio::test]
    async fn probe_sandbox_releases_the_container_on_success() {
        let root = tempfile::tempdir().expect("tempdir");
        let sandbox = StubSandbox::green();

        let r = probe_sandbox(&sandbox, &cfg_in(root.path()), Map::new()).await;

        assert_eq!(r.status, Status::Ok, "{}", r.detail);
        assert!(r.detail.contains("容器已收干净"), "{}", r.detail);
        assert!(sandbox.calls().contains(&"acquire".to_string()), "没起容器");
        assert!(sandbox.calls().contains(&"release".to_string()), "没收容器");
        assert_eq!(r.extra["released"], json!(true));
        assert_eq!(r.extra["sandbox_gone"], json!(true));
        // 证据：四个包的版本号要带回来（Python 那条断言的是 packages.pandas）。
        assert_eq!(r.extra["packages"]["pandas"], json!("2.2.3"));
        assert_eq!(r.extra["packages"]["docx"], json!("1.1.2"));
        assert!(r.extra["elapsed_ms"].is_number(), "{:?}", r.extra);
    }

    /// **第 6 组最值钱的一条** —— 对拍 Python 的
    /// `test_sandbox_releases_container_when_probe_blows_up`：探针炸了，容器照样得收。
    ///
    /// 判据是 `exec` 与 `release` 的**先后顺序**：把 `probe_sandbox` 里那两句调换
    /// （探针一炸就 `return`，不 release），这一条立刻红。自检自己漏容器，比它检出来的
    /// 问题还讨厌。
    #[tokio::test]
    async fn probe_sandbox_releases_the_container_even_when_the_probe_blows_up() {
        let root = tempfile::tempdir().expect("tempdir");
        let sandbox = StubSandbox::green().exec(Err(SandboxError::new(
            SandboxErrorKind::Internal,
            "exec 挂了",
        )));

        let r = probe_sandbox(&sandbox, &cfg_in(root.path()), Map::new()).await;

        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(r.detail.contains("探针没跑成"), "{}", r.detail);
        assert!(
            sandbox.at("acquire").is_some(),
            "没起容器，这条用例就没验到收尾：{:?}",
            sandbox.calls()
        );
        assert_eq!(
            sandbox.at("release"),
            sandbox.at("exec").map(|i| i + 1),
            "探针炸了之后紧接着就该 release：{:?}",
            sandbox.calls()
        );
        assert_eq!(r.extra["released"], json!(true));
    }

    /// 对拍 Python 的 `test_sandbox_fails_when_imports_missing`：镜像缺包就是红，
    /// 缺的那个包名要带出来，重建命令要能直接粘 —— 而且**照样得把容器收掉**。
    #[tokio::test]
    async fn probe_sandbox_fails_when_the_four_imports_are_missing() {
        let root = tempfile::tempdir().expect("tempdir");
        let sandbox = StubSandbox::green().exec(Ok(exec_result(
            1,
            "",
            "ModuleNotFoundError: No module named 'docx'",
        )));

        let r = probe_sandbox(&sandbox, &cfg_in(root.path()), Map::new()).await;

        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(r.detail.contains("docx"), "{}", r.detail);
        assert!(r.fix.contains("docker build"), "{}", r.fix);
        assert_eq!(
            r.extra["released"],
            json!(true),
            "import 没跑通也得把容器收掉"
        );
    }

    /// 退出码是 0 但少了那行标记，同样不算通过 —— 探针的判据是**两条**，不是一条。
    #[tokio::test]
    async fn probe_sandbox_fails_when_the_mark_is_missing_even_at_exit_zero() {
        let root = tempfile::tempdir().expect("tempdir");
        let sandbox = StubSandbox::green().exec(Ok(exec_result(0, "什么都没打印", "")));

        let r = probe_sandbox(&sandbox, &cfg_in(root.path()), Map::new()).await;

        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert_eq!(r.extra["released"], json!(true));
    }

    /// 接管 Python 的 `test_missing_image_names_the_build_command`。
    ///
    /// 语义在 Rust 侧变了：Python 单独查「镜像在不在」，Rust 把镜像与四个 import 合并成
    /// 「真起一个容器跑一遍探针」，于是「镜像不在」表现为 `acquire` 失败。判据不变 ——
    /// 那一行要点名镜像，重建命令要能直接粘。外加一条 Python 没有的：容器都没起来，
    /// 就不该去 release。
    #[tokio::test]
    async fn probe_sandbox_names_the_build_command_when_the_image_is_missing() {
        let root = tempfile::tempdir().expect("tempdir");
        let mut cfg = cfg_in(root.path());
        cfg.sandbox.image = "aite-sandbox:nope".to_string();
        let sandbox = StubSandbox::green().acquire_fails(SandboxError::new(
            SandboxErrorKind::Unavailable,
            "No such image: aite-sandbox:nope",
        ));

        let r = probe_sandbox(&sandbox, &cfg, Map::new()).await;

        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(r.detail.contains("aite-sandbox:nope"), "{}", r.detail);
        assert!(
            r.fix
                .contains("docker build -t aite-sandbox:nope docker/sandbox"),
            "「怎么补」要能直接粘：{}",
            r.fix
        );
        assert!(
            sandbox.at("release").is_none(),
            "容器压根没起来，不该去收：{:?}",
            sandbox.calls()
        );
    }

    /// 收尾失败那条路走到底：`release()` 说没收掉，这一组就红，并把手动收容器的命令
    /// 带上真 task_id。[`release_failure_fails_the_row_even_though_edge_already_forgot_the_id`]
    /// 钉的是判据纯函数，这条钉的是**它真的接在 `probe_sandbox` 上**。
    #[tokio::test]
    async fn probe_sandbox_reports_a_release_failure_as_a_red_row() {
        let root = tempfile::tempdir().expect("tempdir");
        let sandbox = StubSandbox::green().release_fails(SandboxError::new(
            SandboxErrorKind::Internal,
            "Error response from daemon: removal already in progress",
        ));

        let r = probe_sandbox(&sandbox, &cfg_in(root.path()), Map::new()).await;

        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(!r.detail.contains("容器已收干净"), "{}", r.detail);
        assert_eq!(r.extra["released"], json!(false));
        assert!(
            r.extra["release_error"]
                .as_str()
                .is_some_and(|s| s.contains("removal already in progress")),
            "{:?}",
            r.extra
        );
        assert!(
            r.fix.contains("docker rm -f"),
            "「怎么补」要能直接粘：{}",
            r.fix
        );
    }

    fn edge_status(contract: &str, sandbox_ok: bool) -> EdgeStatus {
        EdgeStatus {
            version: "unit-fake-edge".to_string(),
            contract_version: contract.to_string(),
            platform_connected: true,
            reconnect_count: 0,
            sandbox_ok,
            platform: "fake".to_string(),
        }
    }

    /// 对拍 Python 的 `test_docker_daemon_down_is_one_fail_row_not_a_crash`：
    /// daemon 挂了只红一行，并给出怎么补 —— 后面那组照样跑得到（那一半在
    /// `tests/preflight_e2e.rs` 的「一项失败不阻断后面的」里钉）。
    #[test]
    fn edge_status_verdict_turns_a_dead_daemon_into_one_fail_row() {
        let mut extra = Map::new();

        let r = edge_status_verdict(
            "沙箱可用",
            &edge_status(CONTRACT_VERSION, false),
            &mut extra,
        )
        .expect("daemon 挂了就该当场有结论");

        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(r.detail.contains("docker daemon 不可达"), "{}", r.detail);
        assert!(r.fix.contains("Docker Desktop"), "没给出怎么补：{}", r.fix);
        assert_eq!(r.extra["sandbox_ok"], json!(false));
    }

    /// 两边契约版本对不上也是当场红：这时候起容器只会得到更难懂的错。
    #[test]
    fn edge_status_verdict_catches_a_contract_version_skew() {
        let mut extra = Map::new();

        let r = edge_status_verdict("沙箱可用", &edge_status("0.0.0-other", true), &mut extra)
            .expect("版本对不上就该当场有结论");

        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(r.detail.contains(CONTRACT_VERSION), "{}", r.detail);
        assert!(r.detail.contains("0.0.0-other"), "{}", r.detail);
    }

    /// edge 是好的就放行，并把三项证据留在 extra 里。
    #[test]
    fn edge_status_verdict_lets_a_healthy_edge_through() {
        let mut extra = Map::new();

        let verdict =
            edge_status_verdict("沙箱可用", &edge_status(CONTRACT_VERSION, true), &mut extra);

        assert!(verdict.is_none(), "健康的 edge 不该在这里被拦下");
        assert_eq!(extra["sandbox_ok"], json!(true));
        assert_eq!(extra["edge_version"], json!("unit-fake-edge"));
        assert_eq!(extra["edge_contract_version"], json!(CONTRACT_VERSION));
    }

    // ---- 第 7 组：落盘 -----------------------------------------------------

    /// 对拍 Python 的 `test_storage_fails_when_ancestor_not_writable`，外加 §6.6(a)：
    /// 卡住的那层恰好**就是** `repo_root` 时，改之前 `rel()` 出的是空串，打出来是
    /// 「写不下去：data/aite.db（卡在 ）」—— 总管看不出卡在哪一层。
    ///
    /// 别在测试里 chmod 仓库自己的目录，用 tempfile 建一个。
    #[test]
    fn check_storage_fails_and_says_which_layer_is_stuck() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("tempdir");
        let locked = root.path().join("locked");
        std::fs::create_dir(&locked).expect("mkdir");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).expect("chmod");

        // config 用相对路径 → 三个目标都落在 `locked` 下面，而最近的已存在祖先就是
        // `locked` 自己 —— 正是「卡住的那层就是 repo_root」这个现场。
        let r = check_storage(&AiteConfig::default(), &locked);

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).expect("chmod");

        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(r.detail.contains("写不下去"), "{}", r.detail);
        assert!(
            !r.detail.contains("（卡在 ）"),
            "卡在哪一层要说得出来，不能是空串：{}",
            r.detail
        );
        assert!(
            r.detail.contains(&locked.display().to_string()),
            "卡住的那层要指得出来：{}",
            r.detail
        );
        assert_eq!(
            r.extra["storage.sqlite_path"]["existing_ancestor"],
            json!(locked.display().to_string())
        );
    }

    /// 对拍 Python 的 `test_storage_ok_when_parent_missing_but_creatable`：判据是
    /// 「落得下去」而不是「已经在」—— evidence writer 和 store 都会自建目录。
    #[test]
    fn check_storage_is_ok_when_the_parent_is_missing_but_creatable() {
        let root = tempfile::tempdir().expect("tempdir");
        assert!(!root.path().join("data").exists());

        let r = check_storage(&AiteConfig::default(), root.path());

        assert_eq!(r.status, Status::Ok, "{}", r.detail);
        assert!(r.detail.contains("待建"), "{}", r.detail);
        assert_eq!(r.extra["storage.evidence_dir"]["writable"], json!(true));
        assert!(
            !root.path().join("data").exists(),
            "第 7 组只是探测，不该真把 data/ 建出来"
        );
    }
}
