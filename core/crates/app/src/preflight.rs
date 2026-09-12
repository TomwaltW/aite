//! `aite preflight` —— 起飞前自检（对应旧 `scripts/preflight.py`，1211 行）。
//!
//! 七组，挨个点名，每项一行结论 + 非 OK 时一句「怎么补」：
//!
//! | # | 组 | 判据 |
//! |---|---|---|
//! | 1 | 配置可加载 | `config/aite.yaml` 读得出来（不存在退到样例，算 WARN） |
//! | 2 | 环境变量齐 | config 里所有 `*_env` 点到的变量**在不在**（取值一个字都不打） |
//! | 3 | 飞书凭证有效 | 换得到 `tenant_access_token` |
//! | 4 | 飞书身份对得上 | `GET /open-apis/bot/v3/info` 的 `bot.open_id` == `FEISHU_BOT_OPEN_ID` |
//! | 5 | 模型端点通 | 一次最小 chat（`ping`，`max_tokens=16`） |
//! | 6 | 沙箱可用 | 经 edge：daemon 可达 → 起容器 → 四个 import → **一定收掉** |
//! | 7 | 落盘目录可写 | 三个路径的「最近的已存在祖先」写得进去 |
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
    AiteConfig, CONTRACT_VERSION, ExecRequest, Message, ModelConfig, ModelProvider, Role,
    SandboxError, SandboxErrorKind, SandboxNetwork, SandboxSpec,
};
use aite_edge_client::EdgeClient;
use aite_models::{OpenAiCompatModel, cost_of, env_snapshot, resolve_api_key};
use serde_json::{Map, Value, json};

use crate::app::{DEFAULT_CONFIG_PATH, load_config, sandbox_spec_of};

const EXAMPLE_CONFIG_PATH: &str = "config/aite.example.yaml";

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
        if value.len() < MIN_REDACT_LEN || self.items.iter().any(|(v, _)| v == value) {
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

// --------------------------------------------------------------------------
// 1 配置可加载
// --------------------------------------------------------------------------

fn check_config(path: &Path, fell_back: bool) -> (CheckResult, Option<AiteConfig>) {
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
                    format!("HTTP 客户端建不起来：{}", tail(&e.to_string(), 300)),
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
                            tail(&e.to_string(), 300)
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
                    format!("换 tenant_access_token 失败：{}", tail(&e.to_string(), 300)),
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
                format!("查机器人信息失败：{}", tail(&e.to_string(), 300)),
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
                detail: format!("§3.7(b) 探测没跑成：{}", tail(&e.to_string(), 300)),
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
            return fail("model", title, tail(&e.to_string(), 300))
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
            return fail("model", title, tail(&e.to_string(), 300)).with_fix(format!(
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
                format!("连不上 edge：{}", tail(&e.to_string(), 300)),
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
                    tail(&e.to_string(), 300)
                ),
            )
            .with_fix("先起 `aite-edge --config <配置>`；起着的话看它的日志为什么不应答")
            .with_extra(extra);
        }
    };
    extra.insert("edge_version".into(), json!(status.version));
    extra.insert(
        "edge_contract_version".into(),
        json!(status.contract_version),
    );
    extra.insert("sandbox_ok".into(), json!(status.sandbox_ok));
    if status.contract_version != CONTRACT_VERSION {
        return fail(
            "sandbox",
            title,
            format!(
                "两边契约版本不一致：core {CONTRACT_VERSION} vs edge {}",
                status.contract_version
            ),
        )
        .with_fix("两个进程要一起升：重新 `cargo build` + `go build` 之后再起")
        .with_extra(extra);
    }
    if !status.sandbox_ok {
        return fail(
            "sandbox",
            title,
            "edge 连得上，但它报 docker daemon 不可达（EdgeStatus.sandbox_ok=false）",
        )
        .with_fix("启动 Docker Desktop（或 `colima start`），`docker info` 能出东西再重跑；容器化部署要把 /var/run/docker.sock 挂给 edge")
        .with_extra(extra);
    }

    let sandbox = edge.sandbox();
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
    let rel = |p: &Path| -> String {
        p.strip_prefix(repo_root)
            .map(|r| r.display().to_string())
            .unwrap_or_else(|_| p.display().to_string())
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

    let mut notes: Vec<Note> = Vec::new();
    let (cfg_result, cfg) = check_config(&config_path, fell_back);
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

    checks.push(check_env(&cfg, env, opts.offline));

    if opts.offline {
        let reason = "--offline：不碰网络";
        checks.push(skipped("feishu_token", "飞书凭证有效", reason));
        checks.push(skipped("feishu_identity", "飞书身份对得上", reason));
        notes.push(note_passive_listen());
        notes.push(note_history_scope_unverified());
        checks.push(skipped("model", "模型端点通", reason));
        checks.push(skipped("sandbox", "沙箱可用", "--offline：不碰 docker"));
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
}
