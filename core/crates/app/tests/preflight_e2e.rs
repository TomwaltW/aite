//! `aite preflight` 的端到端：`run_checks` → `render_text` / `render_json` 那一层。
//!
//! 接管 Python `tests/scripts/test_preflight.py`（564 行 / 25 条）里**公开面**的那些用例。
//! 私有函数（`check_env` / `check_feishu` / `probe_sandbox` / `arm_redactor` / `check_storage`）
//! 钉在 `src/preflight.rs` 的 `mod tests` 里 —— 集成测试够不着它们。
//!
//! ## 不连网、不起容器
//!
//! * **第 3/4 组**：`Options.feishu_domain` 就是注入口，指到本文件里那台 HTTP 假服务。
//! * **第 5 组**：`model.base_url` 指到同一台，多一条 `/v1/chat/completions` 路由。
//! * **第 6 组**：`edge_socket` 指到临时目录下一个**不存在**的 socket。`EdgeClient::connect`
//!   是懒连接（`core/crates/edge-client/src/lib.rs:44`），所以它当场返回 Ok，紧接着
//!   `status()` 那一发 RPC 立刻失败 —— 这一组必红。它在这一档红是**故意**的：
//!   顺带就把 Python 那条「一项失败不阻断后面的检查」验了。第 6 组自己全绿的样子钉在
//!   `mod tests` 的 `probe_sandbox_releases_the_container_on_success` 里。
//! * **环境变量**：`run_checks` 吃的是 `&HashMap`，测试自己造，不碰进程环境
//!   （`std::env::set_var` 在 Rust 2024 是 `unsafe`，多线程测试进程里碰不得）。
//!
//! ## 假服务为什么要自己写、为什么必须绑 127.0.0.1
//!
//! workspace 的依赖表是冻结的，加 wiremock / httpmock 要停下报告。要的只有「按 path 回
//! 一份 canned JSON」，`tokio::net::TcpListener` 就够 —— 形状照抄
//! `core/crates/models/tests/common/mod.rs`（R5 出于同样的理由也自己搭了一台）。
//!
//! 绑 `127.0.0.1` 不是随手写的：这台机器上 `HTTP_PROXY=HTTPS_PROXY=http://127.0.0.1:7897`，
//! 而 `NO_PROXY=localhost,127.0.0.1,::1`。绑主机名或 `0.0.0.0` 会被代理截胡，测试就变成
//! 看运气。端口也不写死：`bind("127.0.0.1:0")` 拿到**真端口**再传给被测代码，不靠
//! 「睡 100ms 等它起来」（纪律 4：测试不靠真实 sleep）。
//!
//! ## 红线
//!
//! 这个文件里的「密钥」全是一眼认得出是假的常量。真密钥一个字都不许进来。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use aite_app::preflight::{
    Options, Redactor, Report, Status, render_json, render_text, run_checks,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// 假取值
// ---------------------------------------------------------------------------

const FAKE_APP_ID: &str = "cli_e2e_fake_app_id_0123456789";
const FAKE_APP_SECRET: &str = "e2e-fake-app-secret-0123456789";
const FAKE_OPEN_ID: &str = "ou_e2e_fake_bot_open_id_0123456789";
const FAKE_TOKEN: &str = "t-e2e-fake-tenant-access-token-0123456789";
const FAKE_MODEL_KEY: &str = "sk-e2e-fake-model-api-key-0123456789";
const PLACEHOLDER: &str = "的取值已隐去";

const PATH_TOKEN: &str = "/open-apis/auth/v3/tenant_access_token/internal";
const PATH_BOT_INFO: &str = "/open-apis/bot/v3/info";
const PATH_MESSAGES: &str = "/open-apis/im/v1/messages";
const PATH_CHAT: &str = "/v1/chat/completions";

const ALL_TITLES: [&str; 7] = [
    "配置可加载",
    "环境变量齐",
    "飞书凭证有效",
    "飞书身份对得上",
    "模型端点通",
    "沙箱可用",
    "落盘目录可写",
];

// ---------------------------------------------------------------------------
// HTTP 假服务
// ---------------------------------------------------------------------------

struct Stub {
    base: String,
    routes: Arc<Mutex<HashMap<String, (u16, String)>>>,
    hits: Arc<Mutex<Vec<String>>>,
}

impl Stub {
    /// 一条路由都不给的空服务：任何请求都会被记一笔，然后收到 404。
    async fn start(routes: &[(&str, Value)]) -> Self {
        let table: HashMap<String, (u16, String)> = routes
            .iter()
            .map(|(p, v)| ((*p).to_string(), (200u16, v.to_string())))
            .collect();
        let routes = Arc::new(Mutex::new(table));
        let hits = Arc::new(Mutex::new(Vec::new()));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");

        let (r, h) = (routes.clone(), hits.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let (r, h) = (r.clone(), h.clone());
                tokio::spawn(async move {
                    let Some(path) = read_request_path(&mut sock).await else {
                        return;
                    };
                    // query 串不参与路由（§3.7(b) 的探测带 container_id / page_size）。
                    let bare = path.split('?').next().unwrap_or(&path).to_string();
                    h.lock().expect("hits").push(path);
                    let (code, body) = r.lock().expect("routes").get(&bare).cloned().unwrap_or((
                        404,
                        r#"{"code":404,"msg":"假服务没有这条路由"}"#.to_string(),
                    ));
                    let head = format!(
                        "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(body.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });

        Self {
            base: format!("http://{addr}"),
            routes,
            hits,
        }
    }

    /// 开跑之后改一条路由（造「第一发成、第二发败」这类现场）。
    fn set(&self, path: &str, code: u16, body: Value) {
        self.routes
            .lock()
            .expect("routes")
            .insert(path.to_string(), (code, body.to_string()));
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().expect("hits").clone()
    }
}

/// 读到 `\r\n\r\n` 拿请求行，再按 Content-Length 把 body 读完（不读完对端会 reset）。
async fn read_request_path(sock: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > 1 << 20 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let path = lines.next()?.split_whitespace().nth(1)?.to_string();
    let want: usize = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0);
    let mut body_read = buf.len() - head_end - 4;
    while body_read < want {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body_read += n;
    }
    Some(path)
}

// ---------------------------------------------------------------------------
// 上游的 canned 响应
// ---------------------------------------------------------------------------

fn token_ok() -> Value {
    json!({"code": 0, "msg": "ok", "tenant_access_token": FAKE_TOKEN, "expire": 7200})
}

fn bot_info_ok(open_id: &str) -> Value {
    json!({
        "code": 0,
        "msg": "ok",
        "bot": {"open_id": open_id, "app_name": "Aite", "activate_status": 1},
    })
}

fn chat_ok() -> Value {
    json!({
        "id": "chatcmpl-e2e",
        "model": "fake-model",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "pong", "tool_calls": Value::Null},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 7, "completion_tokens": 2, "total_tokens": 9},
    })
}

/// 三条飞书路由 + 一条模型路由，全都答得好好的。
fn happy_routes(open_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        (PATH_TOKEN, token_ok()),
        (PATH_BOT_INFO, bot_info_ok(open_id)),
        (PATH_CHAT, chat_ok()),
    ]
}

// ---------------------------------------------------------------------------
// 被测环境
// ---------------------------------------------------------------------------

/// 四个环境变量都齐的一份快照。`run_checks` 吃的是 `&HashMap`，不碰进程环境。
fn full_env() -> HashMap<String, String> {
    HashMap::from([
        ("FEISHU_APP_ID".to_string(), FAKE_APP_ID.to_string()),
        ("FEISHU_APP_SECRET".to_string(), FAKE_APP_SECRET.to_string()),
        ("FEISHU_BOT_OPEN_ID".to_string(), FAKE_OPEN_ID.to_string()),
        ("AITE_MODEL_API_KEY".to_string(), FAKE_MODEL_KEY.to_string()),
    ])
}

/// 在 `root` 下真写一份 system prompt，返回**相对 root** 的路径。
///
/// 第 1 组现在连 `worker.system_prompt_path` 一起验（`src/preflight.rs` 的 `check_config`），
/// 而契约默认值 `core/crates/worker/prompts/platform.md` 在 tempdir 里当然不存在 ——
/// 不写这一份，每条用例的第 1 组都会 FAIL，而那不是被测行为，是脚手架没跟上。
/// 内容无所谓，读得出来就行：第 1 组只问「在不在」，四条铁律的正文归 worker 那边管。
fn write_prompt(root: &Path) -> String {
    let rel = "prompts/platform.md";
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("有父目录")).expect("建 prompts 目录");
    std::fs::write(&path, "# 假 system prompt（第 1 组只问在不在）\n").expect("写 prompt");
    rel.to_string()
}

/// 一份填得齐的 config，写进 `root`。
///
/// 路径一律**相对** `repo_root`（= tempdir），于是 `storage.*`、`edge_socket` 和
/// `worker.system_prompt_path` 都落在临时目录里 —— 整轨硬约束「一个字节都不许写进仓库的
/// `data/`」（`tests/cold_start_to_delivery.rs:220`、`tests/evidence_on_disk.rs:340`）。
/// 第 7 组的可写探测是真建一个目录再删掉，所以这条不是形式主义。
fn write_config(root: &Path, model_base_url: &str) -> String {
    write_config_with_prompt(root, model_base_url, &write_prompt(root))
}

/// 同上，但 `worker.system_prompt_path` 由调用方说了算 —— 第 1 组那条新判据要拿它造病。
fn write_config_with_prompt(root: &Path, model_base_url: &str, prompt_path: &str) -> String {
    let path = root.join("aite.yaml");
    let text = format!(
        "platform: feishu\n\
         model:\n  \
           provider: openai_compat\n  \
           base_url: {model_base_url}\n  \
           model: fake-model\n\
         worker:\n  \
           system_prompt_path: {prompt_path}\n\
         storage:\n  \
           sqlite_path: data/aite.db\n  \
           evidence_dir: data/evidence\n  \
           artifacts_dir: data/artifacts\n\
         edge:\n  \
           edge_socket: run/nowhere-aite-edge.sock\n"
    );
    std::fs::write(&path, text).expect("写 config");
    path.display().to_string()
}

fn opts(root: &Path, config: String, domain: String) -> Options {
    Options {
        config: Some(config),
        offline: false,
        json: false,
        chat_id: None,
        repo_root: root.to_path_buf(),
        feishu_domain: domain,
    }
}

/// 跑一轮，把两种渲染一起拿回来 —— 红线要在**两种输出**上都成立。
async fn run(opts: &Options, env: &HashMap<String, String>) -> (Report, String, String) {
    let mut redactor = Redactor::new();
    let report = run_checks(opts, env, &mut redactor).await;
    let text = render_text(&report, &redactor);
    let raw_json = render_json(&report, &redactor);
    (report, text, raw_json)
}

fn row_for<'a>(text: &'a str, title: &str) -> &'a str {
    text.lines()
        .find(|l| l.starts_with('[') && l.contains(title))
        .unwrap_or_else(|| panic!("输出里没有「{title}」这一行：\n{text}"))
}

fn rows(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.starts_with('[')).collect()
}

fn status_of(report: &Report, name: &str) -> Status {
    report
        .checks
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("报告里没有 {name} 这一项"))
        .status
}

// ===========================================================================
// 全绿 / 缺项
// ===========================================================================

/// 对拍 Python 的 `test_all_green_exits_zero`，减去第 6 组。
///
/// 第 6 组在这一档必红 —— 它要一个真的 `aite-edge` 在跑，而这里连的是一个不存在的
/// socket。它自己全绿的样子钉在 `mod tests` 的
/// `probe_sandbox_releases_the_container_on_success`。除它以外六行都要是 OK，
/// 而且第 5 组那一发最小 chat **真的发出去了**（Python 那条也断言了这件事）。
#[tokio::test]
async fn every_row_but_the_sandbox_is_green_when_the_upstreams_answer() {
    let root = tempfile::tempdir().expect("tempdir");
    let stub = Stub::start(&happy_routes(FAKE_OPEN_ID)).await;
    let cfg = write_config(root.path(), &format!("{}/v1", stub.base));

    let (report, text, _) = run(&opts(root.path(), cfg, stub.base.clone()), &full_env()).await;

    assert_eq!(rows(&text).len(), 7, "{text}");
    for title in ALL_TITLES {
        if title == "沙箱可用" {
            continue;
        }
        assert!(
            row_for(&text, title).contains("OK "),
            "「{title}」没绿：{}\n{text}",
            row_for(&text, title)
        );
    }
    assert_eq!(status_of(&report, "sandbox"), Status::Fail);
    assert_eq!(report.passed(), 6, "{text}");
    assert!(
        stub.hits().iter().any(|p| p == PATH_CHAT),
        "第 5 组没真发出去那次最小 chat：{:?}",
        stub.hits()
    );
    assert!(
        stub.hits().iter().any(|p| p == PATH_BOT_INFO),
        "{:?}",
        stub.hits()
    );
}

/// 第 1 组现在连 `worker.system_prompt_path` 一起验：配置解析得出来、但它指着一个
/// **不存在的文件**时，第 1 组必须 FAIL —— 不是 WARN、更不是 OK。
///
/// **病史（这条测试守的就是它）**：2026-09-12 `aite preflight --offline` 报「全部没红，
/// 可以起飞」，紧接着 `aite run` 退出码 2 —— 那份 2026-09-10 写的 `config/aite.yaml`
/// 还指着当天被删掉的 Python 树（`aite/worker/prompts/`）。当时七组里**没有一组**碰这个
/// 字段，所以去掉 `--offline` 全跑一遍也救不了（实测第 1 组照样 OK）。
///
/// 四件事一起钉住，少一件这条判据就还是半残的：
/// 1. **FAIL 而不是 WARN** —— 它是硬起飞前提（`build_app` 第 2 步读不到就拒绝起飞）。
/// 2. `report.ok()` 为假 —— 那就是进程退出码 1 的判据（进程级那条在 `cli_smoke.rs`）。
/// 3. 「怎么补」里有**正确路径**和**那条真实病史**，照着做真能修好。
/// 4. **后面六组照跑**：第 1 组红了不阻断后面的（模块头第三条规矩）。
#[tokio::test]
async fn a_system_prompt_that_is_not_there_fails_the_first_row() {
    let root = tempfile::tempdir().expect("tempdir");
    let stub = Stub::start(&happy_routes(FAKE_OPEN_ID)).await;
    // 逐字用总管撞上的那个路径：`fix` 里那句病史只在认出旧 Python 树时才说得出口。
    let cfg = write_config_with_prompt(
        root.path(),
        &format!("{}/v1", stub.base),
        "aite/worker/prompts/platform.md",
    );

    let (report, text, raw_json) =
        run(&opts(root.path(), cfg, stub.base.clone()), &full_env()).await;

    assert_eq!(status_of(&report, "config"), Status::Fail, "{text}");
    assert!(!report.ok(), "第 1 组红了，退出码就该是 1：{text}");

    let row = row_for(&text, "配置可加载");
    assert!(
        row.contains("worker.system_prompt_path"),
        "没点名是哪个配置项：{row}"
    );
    // 解析成的绝对路径要打出来 —— 光说相对路径，人不知道它究竟去哪儿找了。
    assert!(
        row.contains(
            &root
                .path()
                .join("aite/worker/prompts/platform.md")
                .display()
                .to_string()
        ),
        "没打出解析成的绝对路径：{row}"
    );

    let fix = text
        .lines()
        .find(|l| l.contains("怎么补") && l.contains("system_prompt_path"))
        .unwrap_or_else(|| panic!("第 1 组没给「怎么补」：\n{text}"));
    assert!(
        fix.contains("core/crates/worker/prompts/platform.md"),
        "「怎么补」里没有正确路径，照着做修不好：{fix}"
    );
    assert!(
        fix.contains("2026-09-12") && fix.contains("Python 树"),
        "「怎么补」里没说破那条病史（旧配置指着已删的 Python 树）：{fix}"
    );

    // 一项失败不阻断后面的：七行齐，且第 1 组之后确实还在干活。
    assert_eq!(rows(&text).len(), 7, "{text}");
    assert_eq!(status_of(&report, "env"), Status::Ok, "{text}");
    assert_eq!(status_of(&report, "feishu_token"), Status::Ok, "{text}");
    assert_eq!(status_of(&report, "storage"), Status::Ok, "{text}");

    // `--json` 那一面也要看得见，脚本才判得出来。
    let json: Value = serde_json::from_str(&raw_json).expect("--json 必须是 JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["checks"][0]["status"], "fail");
    assert_eq!(json["checks"][0]["extra"]["system_prompt_ok"], false);
}

/// 同一条判据在 `--offline` 下**照样跑**：它不碰网络也不碰 docker，没有理由跳。
///
/// 这条单列是因为总管撞上那次跑的就是 `--offline`（V3 记过的「`--offline` 全绿 ≠ 起得来」
/// 是同一族）。要是哪天有人图省事把第 1 组的新判据挪进「非 offline 才跑」的那半边，
/// 上面那条测试仍然全绿，只有这条会红。
#[tokio::test]
async fn the_system_prompt_row_still_runs_offline() {
    let root = tempfile::tempdir().expect("tempdir");
    let cfg = write_config_with_prompt(
        root.path(),
        "http://127.0.0.1:1/v1",
        "aite/worker/prompts/platform.md",
    );
    let mut o = opts(root.path(), cfg, "http://127.0.0.1:1".to_string());
    o.offline = true;

    let (report, text, _) = run(&o, &full_env()).await;

    assert_eq!(status_of(&report, "config"), Status::Fail, "{text}");
    assert!(!report.ok(), "--offline 下也该是退出码 1：{text}");
    assert!(
        row_for(&text, "配置可加载").contains("worker.system_prompt_path"),
        "{text}"
    );
    // 3/4/5/6 该跳的还跳 —— 这一项跑起来不是靠把 offline 那半边的 skip 拆了。
    assert_eq!(status_of(&report, "model"), Status::Skip, "{text}");
    assert_eq!(status_of(&report, "sandbox"), Status::Skip, "{text}");
}

/// 对拍 Python 的 `test_every_check_still_runs_when_feishu_is_unreachable`：
/// 第 3 组红了，后面几组一个都不能少跑。
#[tokio::test]
async fn a_red_feishu_row_does_not_stop_the_later_checks() {
    let root = tempfile::tempdir().expect("tempdir");
    let stub = Stub::start(&happy_routes(FAKE_OPEN_ID)).await;
    stub.set(
        PATH_TOKEN,
        200,
        json!({"code": 99991663, "msg": "app_secret 不对"}),
    );
    let cfg = write_config(root.path(), &format!("{}/v1", stub.base));

    let (report, text, _) = run(&opts(root.path(), cfg, stub.base.clone()), &full_env()).await;

    assert_eq!(status_of(&report, "feishu_token"), Status::Fail, "{text}");
    assert_eq!(
        status_of(&report, "feishu_identity"),
        Status::Skip,
        "换不到 token，身份无从比对：{text}"
    );
    // 后面三组一个都不能少跑。
    assert_eq!(status_of(&report, "model"), Status::Ok, "{text}");
    assert_eq!(status_of(&report, "storage"), Status::Ok, "{text}");
    assert_eq!(rows(&text).len(), 7, "{text}");
    assert!(
        stub.hits().iter().any(|p| p == PATH_CHAT),
        "第 3 组红了就不跑第 5 组，等于一项失败阻断了后面的：{:?}",
        stub.hits()
    );
}

// ===========================================================================
// --offline
// ===========================================================================

/// 对拍 Python 的 `test_offline_touches_neither_network_nor_docker`：
/// 假服务一条路由都没注册，真发出去就会被记一笔；docker 那边看第 6 组是不是 SKIP。
#[tokio::test]
async fn offline_touches_neither_network_nor_docker() {
    let root = tempfile::tempdir().expect("tempdir");
    let stub = Stub::start(&[]).await;
    let cfg = write_config(root.path(), &format!("{}/v1", stub.base));
    let mut o = opts(root.path(), cfg, stub.base.clone());
    o.offline = true;

    let (report, text, _) = run(&o, &full_env()).await;

    assert!(report.ok(), "--offline 全过才对：{text}");
    assert!(
        stub.hits().is_empty(),
        "--offline 下发了 HTTP：{:?}",
        stub.hits()
    );
    for name in ["feishu_token", "feishu_identity", "model", "sandbox"] {
        assert_eq!(status_of(&report, name), Status::Skip, "{text}");
    }
    for name in ["config", "env", "storage"] {
        assert_ne!(status_of(&report, name), Status::Skip, "{text}");
    }
    assert!(text.contains("--offline 只验了 1/2/7"), "{text}");
}

/// 对拍 Python 的 `test_offline_with_no_credentials_still_exits_zero`：
/// CI 和没凭证的机器上要能跑 —— 缺变量降成 WARN（不拦起飞），但一个名字都不少报。
#[tokio::test]
async fn offline_without_credentials_still_passes() {
    let root = tempfile::tempdir().expect("tempdir");
    let stub = Stub::start(&[]).await;
    let cfg = write_config(root.path(), &format!("{}/v1", stub.base));
    let mut o = opts(root.path(), cfg, stub.base.clone());
    o.offline = true;

    let (report, text, _) = run(&o, &HashMap::new()).await;

    assert!(report.ok(), "{text}");
    assert_eq!(status_of(&report, "env"), Status::Warn, "{text}");
    let env_row = row_for(&text, "环境变量齐");
    for name in full_env().keys() {
        assert!(env_row.contains(name.as_str()), "少报了 {name}：{env_row}");
    }
}

// ===========================================================================
// 红线：密钥取值不外泄
// ===========================================================================

/// 对拍 Python 的 `test_bot_open_id_mismatch_is_fail_without_printing_values`。
///
/// 派单点名「这条尤其要接」：对不上的时候最想打出来的就是那两个 open_id，
/// 正因如此这里要钉死 —— **一个都不许打**，两种渲染都不许。
#[tokio::test]
async fn a_bot_open_id_mismatch_prints_neither_value() {
    let other = "ou_e2e_fake_SOME_OTHER_BOT_9999999999";
    let root = tempfile::tempdir().expect("tempdir");
    let stub = Stub::start(&happy_routes(other)).await;
    let cfg = write_config(root.path(), &format!("{}/v1", stub.base));

    let (report, text, raw_json) =
        run(&opts(root.path(), cfg, stub.base.clone()), &full_env()).await;

    assert_eq!(
        status_of(&report, "feishu_identity"),
        Status::Fail,
        "{text}"
    );
    assert!(
        row_for(&text, "飞书身份对得上").contains("对不上"),
        "{text}"
    );
    for out in [&text, &raw_json] {
        assert!(!out.contains(FAKE_OPEN_ID), "配置侧的 open_id 漏了：{out}");
        assert!(!out.contains(other), "飞书侧的 open_id 漏了：{out}");
    }
    // 该说的还是要说清：不打取值，但得让人认出飞书侧是哪个应用。
    assert!(
        row_for(&text, "飞书身份对得上").contains("飞书侧机器人是 Aite"),
        "{text}"
    );
}

/// 对拍 Python 的 `test_secret_values_never_reach_output` 的**完整形态**：
/// 让上游把凭证**回显进错误消息**（真实网关偶尔就这么干），再端到端过一遍渲染。
///
/// 验的不是「代码里没去打它」，而是 `Redactor` 那道兜底闸真的在工作。
#[tokio::test]
async fn secret_values_never_reach_the_output_even_when_upstream_echoes_them() {
    let root = tempfile::tempdir().expect("tempdir");
    let stub = Stub::start(&[(PATH_TOKEN, token_ok())]).await;
    // 机器人信息接口把 token 和 app_secret 一起回显进 msg。
    stub.set(
        PATH_BOT_INFO,
        200,
        json!({
            "code": 99991663,
            "msg": format!("bad token {FAKE_TOKEN} for secret {FAKE_APP_SECRET}"),
        }),
    );
    // 模型端点把 api key 回显进 401 的响应体。
    stub.set(
        PATH_CHAT,
        401,
        json!({"error": {"message": format!("401 invalid api key {FAKE_MODEL_KEY}")}}),
    );
    let cfg = write_config(root.path(), &format!("{}/v1", stub.base));

    let (report, text, raw_json) =
        run(&opts(root.path(), cfg, stub.base.clone()), &full_env()).await;

    assert!(!report.ok(), "{text}");
    for out in [&text, &raw_json] {
        for (secret, what) in [
            (FAKE_TOKEN, "tenant_access_token"),
            (FAKE_APP_SECRET, "app_secret"),
            (FAKE_APP_ID, "app_id"),
            (FAKE_OPEN_ID, "bot_open_id"),
            (FAKE_MODEL_KEY, "model api key"),
        ] {
            assert!(!out.contains(secret), "{what} 漏进输出了：{out}");
        }
        // 反过来钉一条：确实是被抹掉的，而不是这几段文本压根没走到输出。
        assert!(out.contains(PLACEHOLDER), "{out}");
    }
}

/// §6.3 口子①：配置**读不出来**那条早退路径上，`Redactor` 原本是空的。
///
/// `check_config` 的 FAIL detail 回显的是 serde_yaml 的错误 —— 它会把出错的标量**原样**
/// 打出来。真机上最典型的形态就是这个：用户把 app_secret 粘进了 `config/aite.yaml`，
/// 于是自检一边在「怎么补」里教他「密钥只写环境变量名，不写取值」，一边把取值打在
/// 屏幕上。
///
/// **把 `run_checks` 开头那句 `arm_redactor(..., &env_var_names(&AiteConfig::default()), ...)`
/// 拆掉，这一条就红。**（那五个 `redactor.add()` 调用点全在第 3/4/5 组里，这条路上
/// 一个都到不了。）
#[tokio::test]
async fn a_config_that_echoes_a_secret_does_not_leak_it() {
    let root = tempfile::tempdir().expect("tempdir");
    let bad = root.path().join("bad.yaml");
    // 出错的标量**恰好就是**环境变量里那个凭证的取值。
    std::fs::write(
        &bad,
        format!("feishu:\n  history_window: {FAKE_APP_SECRET}\n"),
    )
    .expect("写 config");
    let mut o = opts(
        root.path(),
        bad.display().to_string(),
        "http://127.0.0.1:1".to_string(),
    );
    o.offline = true;

    let (report, text, raw_json) = run(&o, &full_env()).await;

    assert_eq!(status_of(&report, "config"), Status::Fail, "{text}");
    for out in [&text, &raw_json] {
        assert!(
            !out.contains(FAKE_APP_SECRET),
            "配置读不出来那条路上取值原样漏出来了：{out}"
        );
        assert!(out.contains(PLACEHOLDER), "{out}");
    }
}

/// §6.4 口子②在 `run_checks` 那一层：真 config 把 `*_env` 指到**默认之外**的名字上时，
/// 脱敏也得认得它 —— 靠的是读到真 config 之后那次 `arm_redactor(&env_var_names(&cfg))`。
///
/// 走 `--offline`：这一档下第 3/4/5 组全 skip，五个 `redactor.add()` 调用点一个都到不了，
/// 所以只要输出里那个取值被抹了，抹它的就只能是装料那一步。
#[tokio::test]
async fn a_custom_env_var_name_is_still_redacted() {
    let custom = "AITE_E2E_FAKE_CUSTOM_KEY_ENV";
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("aite.yaml");
    // 这条用例关心的是脱敏，不是第 1 组；prompt 得真写一份，否则第 1 组会因为
    // `worker.system_prompt_path` 指不到而 FAIL，把下面那条前置断言打红。
    let prompt = write_prompt(root.path());
    std::fs::write(
        &path,
        format!(
            "model:\n  api_key_env: {custom}\nworker:\n  system_prompt_path: {prompt}\n\
             storage:\n  sqlite_path: data/aite.db\n  \
             evidence_dir: data/evidence\n  artifacts_dir: data/artifacts\n"
        ),
    )
    .expect("写 config");
    let env = HashMap::from([(custom.to_string(), FAKE_MODEL_KEY.to_string())]);
    let mut o = opts(
        root.path(),
        path.display().to_string(),
        "http://127.0.0.1:1".to_string(),
    );
    o.offline = true;

    let mut redactor = Redactor::new();
    let report = run_checks(&o, &env, &mut redactor).await;

    assert_eq!(status_of(&report, "config"), Status::Ok);
    // 第 2 组会自动报这个新名字（`env_var_names` 是泛化扫的）……
    assert!(
        render_text(&report, &redactor).contains(custom),
        "第 2 组没报出自定义的变量名"
    );
    // ……脱敏也必须跟着认得它的取值。
    let echoed = redactor.scrub(&format!("上游回显了 {FAKE_MODEL_KEY}"));
    assert!(!echoed.contains(FAKE_MODEL_KEY), "{echoed}");
    assert!(
        echoed.contains(custom),
        "占位符要说清抹的是哪一项：{echoed}"
    );
}

// ===========================================================================
// 配置坏掉 / --json 形状 / --chat-id
// ===========================================================================

/// 对拍 Python 的 `test_bad_config_fails_but_still_reports_seven_rows`：
/// 第 1 组红了，剩下六项照样各占一行，说清为什么 —— 七行一行都不能少。
#[tokio::test]
async fn a_broken_config_still_reports_seven_rows() {
    let root = tempfile::tempdir().expect("tempdir");
    let bad = root.path().join("bad.yaml");
    std::fs::write(&bad, "platform: 不存在的平台\n").expect("写 config");
    let mut o = opts(
        root.path(),
        bad.display().to_string(),
        "http://127.0.0.1:1".to_string(),
    );
    o.offline = true;

    let (report, text, _) = run(&o, &full_env()).await;

    assert!(!report.ok(), "{text}");
    assert_eq!(status_of(&report, "config"), Status::Fail, "{text}");
    assert_eq!(rows(&text).len(), 7, "{text}");
    for title in ALL_TITLES.iter().skip(1) {
        assert!(
            row_for(&text, title).contains("SKIP"),
            "「{title}」该说清为什么没跑：{text}"
        );
    }
    assert!(text.contains("汇总："), "汇总还是要打出来：{text}");
}

/// 对拍 Python 的 `test_json_output_shape`：七项齐、顺序固定、每项五个字段、
/// §3.7 那两条提示行在且如实标成没核实。
#[tokio::test]
async fn json_output_shape() {
    let root = tempfile::tempdir().expect("tempdir");
    let stub = Stub::start(&[]).await;
    let cfg = write_config(root.path(), &format!("{}/v1", stub.base));
    let mut o = opts(root.path(), cfg, stub.base.clone());
    o.offline = true;

    let (_, _, raw_json) = run(&o, &full_env()).await;
    let payload: Value = serde_json::from_str(&raw_json).expect("合法 JSON");

    assert_eq!(payload["ok"], json!(true));
    assert_eq!(payload["total"], json!(7));
    let names: Vec<&str> = payload["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .map(|c| c["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        [
            "config",
            "env",
            "feishu_token",
            "feishu_identity",
            "model",
            "sandbox",
            "storage"
        ]
    );
    for check in payload["checks"].as_array().expect("checks") {
        for field in ["name", "title", "status", "detail", "fix", "extra"] {
            assert!(check.get(field).is_some(), "{field} 缺了：{check}");
        }
        let status = check["status"].as_str().expect("status");
        assert!(
            ["ok", "fail", "warn", "skip"].contains(&status),
            "status 是个没见过的值：{status}"
        );
    }
    let notes: Vec<&str> = payload["notes"]
        .as_array()
        .expect("notes")
        .iter()
        .map(|n| n["name"].as_str().expect("name"))
        .collect();
    assert_eq!(notes.len(), 2, "{raw_json}");
    assert!(notes.contains(&"feishu_passive_listen"), "{raw_json}");
    assert!(notes.contains(&"feishu_group_history_scope"), "{raw_json}");
    for note in payload["notes"].as_array().expect("notes") {
        let status = note["status"].as_str().expect("status");
        assert!(
            ["unverified", "unverifiable"].contains(&status),
            "没核实的事不许标成核实了：{status}"
        );
    }
}

/// 对拍 Python 的 `test_json_carries_model_and_sandbox_evidence` 的模型那一半
/// （沙箱那一半在 `mod tests` 的 `probe_sandbox_releases_the_container_on_success`）。
#[tokio::test]
async fn json_carries_model_evidence() {
    let root = tempfile::tempdir().expect("tempdir");
    let stub = Stub::start(&happy_routes(FAKE_OPEN_ID)).await;
    let cfg = write_config(root.path(), &format!("{}/v1", stub.base));

    let (_, _, raw_json) = run(&opts(root.path(), cfg, stub.base.clone()), &full_env()).await;
    let payload: Value = serde_json::from_str(&raw_json).expect("合法 JSON");
    let model = payload["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|c| c["name"] == json!("model"))
        .expect("model 那一项")
        .clone();

    assert_eq!(model["status"], json!("ok"), "{model}");
    assert_eq!(model["extra"]["input_tokens"], json!(7), "{model}");
    assert_eq!(model["extra"]["output_tokens"], json!(2), "{model}");
    assert_eq!(model["extra"]["finish_reason"], json!("stop"), "{model}");
    assert!(model["extra"]["latency_ms"].is_number(), "{model}");
    // 没配价格时也要有个数，而不是缺字段。
    assert!(model["extra"]["cost_cny"].is_number(), "{model}");
}

/// 对拍 Python 的 `test_chat_id_probes_group_history`：给了 `--chat-id` 就实测一次群历史，
/// 把 §3.7(b) 从「未核实」变成有结论。
#[tokio::test]
async fn a_chat_id_turns_the_history_note_into_a_verdict() {
    let root = tempfile::tempdir().expect("tempdir");
    let mut routes = happy_routes(FAKE_OPEN_ID);
    routes.push((
        PATH_MESSAGES,
        json!({"code": 0, "msg": "ok", "data": {"items": [{"message_id": "om_1"}]}}),
    ));
    let stub = Stub::start(&routes).await;
    let cfg = write_config(root.path(), &format!("{}/v1", stub.base));
    let mut o = opts(root.path(), cfg, stub.base.clone());
    o.chat_id = Some("oc_e2e_fake_chat".to_string());

    let (report, text, _) = run(&o, &full_env()).await;

    let note = report
        .notes
        .iter()
        .find(|n| n.name == "feishu_group_history_scope")
        .expect("§3.7(b) 那条提示行");
    assert_eq!(note.status, "verified", "{}", note.detail);
    assert!(note.detail.contains("读得到"), "{}", note.detail);
    assert!(
        stub.hits().iter().any(|p| p.starts_with(PATH_MESSAGES)),
        "没真去探一次群历史：{:?}",
        stub.hits()
    );
    assert!(text.contains("§3.7 待核实"), "提示行要渲出来：{text}");
}

/// 对拍 Python 的 `test_chat_id_probe_reports_permission_failure`：
/// 读不到也是**有结论**（verified），而且它只是提示行 —— 不改判据。
#[tokio::test]
async fn a_chat_id_probe_reports_a_permission_failure_without_failing_the_run() {
    let root = tempfile::tempdir().expect("tempdir");
    let mut routes = happy_routes(FAKE_OPEN_ID);
    routes.push((
        PATH_MESSAGES,
        json!({"code": 99991672, "msg": "permission denied"}),
    ));
    let stub = Stub::start(&routes).await;
    let cfg = write_config(root.path(), &format!("{}/v1", stub.base));
    let mut o = opts(root.path(), cfg, stub.base.clone());
    o.chat_id = Some("oc_e2e_fake_chat".to_string());

    let (report, text, _) = run(&o, &full_env()).await;

    let note = report
        .notes
        .iter()
        .find(|n| n.name == "feishu_group_history_scope")
        .expect("§3.7(b) 那条提示行");
    assert_eq!(note.status, "verified", "{}", note.detail);
    assert!(note.detail.contains("读**不到**"), "{}", note.detail);
    // 提示行不进判据：第 3/4 组该绿还是绿。
    assert_eq!(status_of(&report, "feishu_token"), Status::Ok, "{text}");
    assert_eq!(status_of(&report, "feishu_identity"), Status::Ok, "{text}");
}

// ===========================================================================
// 入口
// ===========================================================================

/// 对拍 Python 的 `test_help_exits_zero`。
#[test]
fn help_exits_zero() {
    assert_eq!(aite_app::preflight::run(vec!["--help".to_string()]), 0);
    assert_eq!(aite_app::preflight::run(vec!["-h".to_string()]), 0);
}

/// 参数敲错不是「自检没过」（1），是「你没说清要我干什么」（2）。
#[test]
fn a_bad_argument_exits_two() {
    assert_eq!(aite_app::preflight::run(vec!["--nope".to_string()]), 2);
    assert_eq!(aite_app::preflight::run(vec!["--config".to_string()]), 2);
    assert_eq!(aite_app::preflight::run(vec!["--chat-id".to_string()]), 2);
}
