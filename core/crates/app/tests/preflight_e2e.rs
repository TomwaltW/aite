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
use aite_contracts::SessionStore;
use aite_store::SqliteSessionStore;
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

/// 契约默认的 `storage.sqlite_path`，也是「怎么补」里那句「该改成什么」。
/// 与 `src/preflight.rs` 的 `DEFAULT_SQLITE_PATH` 同值（那边拿契约 + 样例两头钉着）。
const DEFAULT_SQLITE_PATH: &str = "data/aite.db";
/// 造病用：内容明摆着不是 SQLite 的一个文件。
const NOT_A_DATABASE: &[u8] = b"this is definitely not a sqlite database";
/// 脚手架默认的 `model.model`。第 ① 组 2026-09-15 起要它非空，所以它得有个值 ——
/// 只有那条 **model 段** 的用例会把它换成空串来造病。
const DEFAULT_MODEL_NAME: &str = "fake-model";

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

/// 同上，但 `worker.system_prompt_path` 由调用方说了算 —— 第 1 组那条 prompt 判据要拿它造病。
fn write_config_with_prompt(root: &Path, model_base_url: &str, prompt_path: &str) -> String {
    write_config_full(root, model_base_url, prompt_path, "feishu", "openai_compat")
}

/// 同上，但 `platform` / `model.provider` 由调用方说了算 —— 第 1 组那条**注入**判据
/// （`platform: fake` / `provider: scripted` 只能被注入着用）要拿它造病。
fn write_config_with_choices(
    root: &Path,
    model_base_url: &str,
    platform: &str,
    provider: &str,
) -> String {
    let prompt = write_prompt(root);
    write_config_full(root, model_base_url, &prompt, platform, provider)
}

/// 同上，但 `storage.sqlite_path` 由调用方说了算 —— 第 1 组那条**库**判据
/// （已经在那儿的那个文件当不当得了库）要拿它造病。
fn write_config_with_sqlite(root: &Path, model_base_url: &str, sqlite_path: &str) -> String {
    let prompt = write_prompt(root);
    write_config_full_with_sqlite(
        root,
        model_base_url,
        DEFAULT_MODEL_NAME,
        &prompt,
        "feishu",
        "openai_compat",
        sqlite_path,
    )
}

/// 同上，但 `model.model` 由调用方说了算 —— 第 ① 组那条 **model 段填得齐** 的判据
/// （2026-09-15 折进来的第四件）要拿它造病。
fn write_config_with_model(root: &Path, model_base_url: &str, model_name: &str) -> String {
    let prompt = write_prompt(root);
    write_config_full_with_sqlite(
        root,
        model_base_url,
        model_name,
        &prompt,
        "feishu",
        "openai_compat",
        DEFAULT_SQLITE_PATH,
    )
}

fn write_config_full(
    root: &Path,
    model_base_url: &str,
    prompt_path: &str,
    platform: &str,
    provider: &str,
) -> String {
    write_config_full_with_sqlite(
        root,
        model_base_url,
        DEFAULT_MODEL_NAME,
        prompt_path,
        platform,
        provider,
        DEFAULT_SQLITE_PATH,
    )
}

fn write_config_full_with_sqlite(
    root: &Path,
    model_base_url: &str,
    model_name: &str,
    prompt_path: &str,
    platform: &str,
    provider: &str,
    sqlite_path: &str,
) -> String {
    let path = root.join("aite.yaml");
    let text = format!(
        "platform: {platform}\n\
         model:\n  \
           provider: {provider}\n  \
           base_url: \"{model_base_url}\"\n  \
           model: \"{model_name}\"\n\
         worker:\n  \
           system_prompt_path: {prompt_path}\n\
         storage:\n  \
           sqlite_path: {sqlite_path}\n  \
           evidence_dir: data/evidence\n  \
           artifacts_dir: data/artifacts\n\
         edge:\n  \
           edge_socket: run/nowhere-aite-edge.sock\n"
    );
    std::fs::write(&path, text).expect("写 config");
    path.display().to_string()
}

/// 对拍专用：`storage.*` 与 `worker.system_prompt_path` 全写**绝对路径**。
///
/// 上面那几个 helper 写的是相对路径，preflight 按 `Options.repo_root`（= tempdir）解析，
/// 落点干净。但 `build_app` 那一侧不一样：`prepare_storage` 拿的是**裸相对路径**
/// （`Path::new(&cfg.evidence_dir)`，相对进程 cwd），`require_system_prompt` 也是相对 cwd ——
/// 直接把上面那份喂给 `build_app`，`data/` 会建到 `core/crates/app/` 底下去，
/// 撞上整轨那条硬约束「一个字节都不许写进仓库的 `data/`」。所以对拍这一档全用绝对路径。
fn write_config_absolute(root: &Path, platform: &str, provider: &str) -> String {
    let prompt = root.join("prompts/platform.md");
    std::fs::create_dir_all(prompt.parent().expect("有父目录")).expect("建 prompts 目录");
    std::fs::write(&prompt, "# 假 system prompt\n").expect("写 prompt");
    let path = root.join("aite.yaml");
    let text = format!(
        "platform: {platform}\n\
         model:\n  \
           provider: {provider}\n  \
           base_url: http://127.0.0.1:1/v1\n  \
           model: fake-model\n\
         worker:\n  \
           system_prompt_path: {}\n\
         storage:\n  \
           sqlite_path: {}\n  \
           evidence_dir: {}\n  \
           artifacts_dir: {}\n\
         edge:\n  \
           edge_socket: run/nowhere-aite-edge.sock\n",
        prompt.display(),
        root.join("data/aite.db").display(),
        root.join("data/evidence").display(),
        root.join("data/artifacts").display(),
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

// ===========================================================================
// 第 1 组的注入判据（`platform: fake` / `model.provider: scripted`）
// ===========================================================================

/// `platform: fake` → 第 1 组 FAIL，第 2/3/4 组 SKIP，后面照跑。
///
/// **病史（这条测试守的就是它）**：2026-09-13 总管拿一份 `platform: fake` 的配置跑
/// `aite preflight --offline`，得到「全部没红，可以起飞。」退出码 0；紧接着 `aite run`
/// 退出码 2（`build_app` 明文拒绝：fake 必须由调用方注入平台实现）。与 prompt 那条
/// **同形状**：七组里没有一组问「这个取值自己起得来吗」。
///
/// 同一份配置还暴露了第二件事：它照样在要飞书凭证（第 2/3/4 组全红），而 **fake 平台
/// 压根不连飞书** —— 判据对它毫无意义。所以这一档下那三组 SKIP。
///
/// 五件事一起钉住：
/// 1. **FAIL 而不是 WARN** —— `aite run` 那边是硬拒绝，退出码 2，不是「能飞但有风险」。
/// 2. `report.ok()` 为假 —— 退出码 1 的判据（进程级那条在 `cli_smoke.rs`）。
/// 3. **2/3/4 组 SKIP，且理由写在那一行里**（不是默默跳过，口径照 `--offline：不碰网络`）。
/// 4. 「怎么补」三件事齐：当前值、该改成什么、以及 fake 是留给谁用的。
/// 5. **后面几组照跑**：第 1 组红了不阻断后面的（模块头第三条规矩）。
#[tokio::test]
async fn a_fake_platform_fails_the_first_row_and_skips_the_feishu_rows() {
    let root = tempfile::tempdir().expect("tempdir");
    let stub = Stub::start(&happy_routes(FAKE_OPEN_ID)).await;
    let cfg = write_config_with_choices(
        root.path(),
        &format!("{}/v1", stub.base),
        "fake",
        "openai_compat",
    );

    let (report, text, raw_json) =
        run(&opts(root.path(), cfg, stub.base.clone()), &full_env()).await;

    assert_eq!(status_of(&report, "config"), Status::Fail, "{text}");
    assert!(!report.ok(), "fake 起不来，退出码就该是 1：{text}");

    let row = row_for(&text, "配置可加载");
    assert!(row.contains("platform=fake"), "没点名是哪个取值：{row}");
    assert!(
        row.contains("注入") && row.contains("aite run"),
        "没说清病在哪（要注入，而 aite run 不注入）：{row}"
    );

    let fix = text
        .lines()
        .find(|l| l.contains("怎么补") && l.contains("platform"))
        .unwrap_or_else(|| panic!("第 1 组没给「怎么补」：\n{text}"));
    // 连右括号一起断：光断 `contains("feishu")` 的话，把常量敲成 `feishuu` 也照样绿。
    assert!(
        fix.contains("改成 feishu（"),
        "「怎么补」里没说该改成什么，照着做修不好：{fix}"
    );
    assert!(
        fix.contains("评测") && fix.contains("§3.1"),
        "「怎么补」里没说 fake 是留给谁用的：{fix}"
    );

    // 2/3/4 组：SKIP，且各自那一行要说得出为什么。
    for (name, title) in [
        ("env", "环境变量齐"),
        ("feishu_token", "飞书凭证有效"),
        ("feishu_identity", "飞书身份对得上"),
    ] {
        assert_eq!(status_of(&report, name), Status::Skip, "{title}：{text}");
        assert!(
            row_for(&text, title).contains("platform=fake"),
            "{title} 默默跳过了，没说为什么：{text}"
        );
    }

    // 一项失败不阻断后面的：七行齐，第 5/7 组照跑（第 6 组连的是不存在的 socket，必红）。
    assert_eq!(rows(&text).len(), 7, "{text}");
    assert_eq!(status_of(&report, "model"), Status::Ok, "{text}");
    assert_eq!(status_of(&report, "storage"), Status::Ok, "{text}");

    // 飞书那三组是 SKIP 而不是「查了没查出问题」：飞书那几个端点一发都不许出去。
    // （第 5 组的 `/v1/chat/completions` 不在此列 —— 模型跟 platform 取值无关，照跑。）
    let feishu_hits: Vec<String> = stub
        .hits()
        .into_iter()
        .filter(|p| p.starts_with("/open-apis/"))
        .collect();
    assert!(
        feishu_hits.is_empty(),
        "fake 平台不该碰飞书，却发了：{feishu_hits:?}"
    );

    let json: Value = serde_json::from_str(&raw_json).expect("--json 必须是 JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["checks"][0]["status"], "fail");
    assert_eq!(json["checks"][0]["extra"]["needs_injection"], true);
}

/// `model.provider: scripted` → 同一处判断、同一种收场，但**不碰飞书那三组**。
///
/// 分两半钉：scripted 是模型那一侧的事，`platform` 还是 `feishu`，所以第 2/3/4 组
/// 该跑照跑 —— 把 SKIP 的条件写成「第 1 组红了就跳」会让这条红。
///
/// 第 5 组那句 WARN（`provider=scripted，没有真端点可探`）是**另一件事**、不冲突：
/// 它管「端点通不通」，第 1 组管「这份配置起不起得来」。两条并存，各说各的本分。
#[tokio::test]
async fn a_scripted_model_provider_fails_the_first_row_but_leaves_feishu_alone() {
    let root = tempfile::tempdir().expect("tempdir");
    let stub = Stub::start(&happy_routes(FAKE_OPEN_ID)).await;
    let cfg = write_config_with_choices(
        root.path(),
        &format!("{}/v1", stub.base),
        "feishu",
        "scripted",
    );

    let (report, text, _) = run(&opts(root.path(), cfg, stub.base.clone()), &full_env()).await;

    assert_eq!(status_of(&report, "config"), Status::Fail, "{text}");
    assert!(!report.ok(), "{text}");
    let row = row_for(&text, "配置可加载");
    assert!(
        row.contains("model.provider=scripted") && row.contains("注入"),
        "{row}"
    );
    let fix = text
        .lines()
        .find(|l| l.contains("怎么补") && l.contains("model.provider"))
        .unwrap_or_else(|| panic!("第 1 组没给「怎么补」：\n{text}"));
    assert!(
        fix.contains("改成 openai_compat（") && fix.contains("§3.1"),
        "「怎么补」缺「该改成什么」或「留给谁用的」：{fix}"
    );

    // platform 还是 feishu：那三组照跑，一组都不许跳。
    assert_eq!(status_of(&report, "env"), Status::Ok, "{text}");
    assert_eq!(status_of(&report, "feishu_token"), Status::Ok, "{text}");
    assert_eq!(status_of(&report, "feishu_identity"), Status::Ok, "{text}");
    // 第 5 组照旧给它那句 WARN，不被第 1 组抢走。
    assert_eq!(status_of(&report, "model"), Status::Warn, "{text}");
    assert_eq!(rows(&text).len(), 7, "{text}");
}

/// 两个取值都正常时**一个字都不变** —— 新判据不许误伤好配置。
///
/// `every_row_but_the_sandbox_is_green_when_the_upstreams_answer` 已经覆盖了全绿这一档，
/// 这条单钉那个 `extra` 字段：判据写成「只要有 platform 字段就 FAIL」之类的蠢样子，
/// 上面那条也会红，但这条能一眼指出病在注入判据上。
#[tokio::test]
async fn a_normal_config_does_not_trip_the_injection_check() {
    let root = tempfile::tempdir().expect("tempdir");
    let stub = Stub::start(&happy_routes(FAKE_OPEN_ID)).await;
    let cfg = write_config_with_choices(
        root.path(),
        &format!("{}/v1", stub.base),
        "feishu",
        "openai_compat",
    );

    let (report, text, raw_json) =
        run(&opts(root.path(), cfg, stub.base.clone()), &full_env()).await;

    assert_eq!(status_of(&report, "config"), Status::Ok, "{text}");
    let json: Value = serde_json::from_str(&raw_json).expect("--json 必须是 JSON");
    assert_eq!(json["checks"][0]["extra"]["needs_injection"], false);
}

/// 注入判据在 `--offline` 下**照样跑** —— 这是它最要紧的地方。
///
/// 总管两次撞上「全绿而起不来」跑的都是 `--offline`。第 5 组那句 WARN 救不了 scripted
/// 这一档（`--offline` 把第 5 组整个跳了），所以判据必须落在第 1 组、且与 offline 无关。
/// 谁哪天图省事把它挪进「非 offline 才跑」的那半边，上面两条仍然全绿，只有这条会红。
#[tokio::test]
async fn the_injection_rows_still_run_offline() {
    let root = tempfile::tempdir().expect("tempdir");
    let cfg = write_config_with_choices(root.path(), "http://127.0.0.1:1/v1", "fake", "scripted");
    let mut o = opts(root.path(), cfg, "http://127.0.0.1:1".to_string());
    o.offline = true;

    let (report, text, _) = run(&o, &full_env()).await;

    assert_eq!(status_of(&report, "config"), Status::Fail, "{text}");
    assert!(!report.ok(), "--offline 下也该是退出码 1：{text}");
    // 两条都犯了就**一次报齐**，别让人改完一条重跑才发现还有一条。
    let row = row_for(&text, "配置可加载");
    assert!(
        row.contains("platform=fake") && row.contains("model.provider=scripted"),
        "两条只报了一条：{row}"
    );
    assert_eq!(rows(&text).len(), 7, "{text}");
}

/// **两边口径对拍**：第 1 组拒绝的那份配置，真 `build_app` 也必须拒绝。
///
/// prompt 那条判据是靠**复用同一个函数**（`load_system_prompt`）保证两边一致的；注入这条
/// 没有函数可复用（判据是两个不等式），所以改用**行为对拍**：同一份 yaml，preflight 读它
/// 说 FAIL，`build_app` 读它必须 `StartupError`。哪天 `app.rs` 那两条改了口径而
/// `preflight.rs` 没跟上，这条会红。
///
/// 顺带钉住 [`injection_fault`] 依赖的那条前提：**`aite run` 不注入任何实现**。这里传的
/// 就是 `Injections::default()`（`cli.rs` 里那个唯一的 `build_app` 调用点传的也是它），
/// 哪天真给 `aite run` 加了注入开关，preflight 这条判据的地基就塌了 —— 那时候该红的是这条。
///
/// `scripted` 那一档会先去连 edge（`check_contract_version` 最多 5 次、每次隔 1s），
/// 所以它比别的测试慢几秒 —— 那是产品代码的重试预算，不是这里在 sleep。
#[tokio::test]
async fn build_app_really_refuses_what_the_first_check_refuses() {
    for (platform, provider, keyword) in [
        ("fake", "openai_compat", "platform=fake"),
        ("feishu", "scripted", "model.provider=scripted"),
    ] {
        let root = tempfile::tempdir().expect("tempdir");
        let cfg_path = write_config_absolute(root.path(), platform, provider);

        // preflight 这一侧：第 1 组 FAIL。
        let mut o = opts(
            root.path(),
            cfg_path.clone(),
            "http://127.0.0.1:1".to_string(),
        );
        o.offline = true;
        let (report, text, _) = run(&o, &full_env()).await;
        assert_eq!(
            status_of(&report, "config"),
            Status::Fail,
            "preflight 放行了 {platform}/{provider}：{text}"
        );
        assert!(row_for(&text, "配置可加载").contains(keyword), "{text}");

        // build_app 那一侧：同一份配置，必须拒绝起飞。
        let config = aite_app::load_config(&cfg_path).expect("配置本身是好的");
        let outcome = aite_app::build_app(
            config,
            aite_app::Injections {
                repo_root: Some(root.path().to_path_buf()),
                ..aite_app::Injections::default()
            },
        )
        .await;
        let err = match outcome {
            Ok(_) => panic!("preflight 说 {platform}/{provider} 起不来，build_app 却放行了"),
            Err(e) => e,
        };
        assert!(
            err.0.contains(keyword) && err.0.contains("注入"),
            "两边说的不是同一件事：{err}"
        );
    }
}

// ===========================================================================
// 第 1 组的 model 段判据（BB6，2026-09-15）
// ===========================================================================

/// `model` 段少填一样 → 第 1 组 FAIL，**而且 `--offline` 下就拦得住**。
///
/// **病史（这条测试守的就是它）**：2026-09-15 实跑对拍出来的三个同族口子。三样各缺一个的
/// 配置，`preflight --offline` 都是「汇总：OK 2 · WARN 1 · FAIL 0 · SKIP 4 /
/// 全部没红，可以起飞。」退出码 0，紧接着 `aite run` 退出码 2：
///
/// | 缺什么 | `aite run` 说什么 |
/// |---|---|
/// | `model.base_url` | `模型配置不完整：ModelConfig.base_url 是空的` |
/// | `model.model` | `模型配置不完整：ModelConfig.model 是空的` |
/// | `AITE_MODEL_API_KEY` | `模型配置不完整：环境变量 AITE_MODEL_API_KEY 没设置或为空` |
///
/// 前两条 X1 / Y2 两轮都记过「归第 5 组，被 `--offline` 跳过」；第三条谁都没记过 ——
/// 第 2 组在 `--offline` 下把缺变量降成 WARN，于是它连个红都没有。三条都不需要联网，
/// 所以 2026-09-15 起归第 ① 组。**`--offline` 下照跑才是要紧的地方**：谁哪天把这条判据
/// 挪回「非 offline 才跑」的那半边，只有这里会红。
#[tokio::test]
async fn a_blank_model_section_fails_the_first_row_even_offline() {
    for (base_url, model_name, env, keyword) in [
        ("", DEFAULT_MODEL_NAME, full_env(), "model.base_url"),
        ("http://127.0.0.1:1/v1", "", full_env(), "model.model"),
        (
            "http://127.0.0.1:1/v1",
            DEFAULT_MODEL_NAME,
            HashMap::new(),
            "AITE_MODEL_API_KEY",
        ),
    ] {
        let root = tempfile::tempdir().expect("tempdir");
        let cfg = write_config_with_model(root.path(), base_url, model_name);
        let mut o = opts(root.path(), cfg, "http://127.0.0.1:1".to_string());
        o.offline = true;

        let (report, text, raw_json) = run(&o, &env).await;

        assert_eq!(
            status_of(&report, "config"),
            Status::Fail,
            "缺 {keyword} 却放行了：{text}"
        );
        let row = row_for(&text, "配置可加载");
        assert!(row.contains(keyword), "第 ① 组没点名缺的是哪一样：{row}");
        assert!(
            !report.ok(),
            "第 1 组红了整份自检就该红（退出码 1）：{text}"
        );
        assert!(
            !text.contains("全部没红，可以起飞"),
            "起不来还说可以起飞 —— 这正是本轨要治的那条：\n{text}"
        );
        // 第 5 组在 `--offline` 下照旧 SKIP —— 本轨没有把它搬到离线来，只是把它里头
        // 「不联网也判得出来」的那半句借给了第 ① 组。
        assert_eq!(status_of(&report, "model"), Status::Skip, "{text}");
        // 一项失败不阻断后面的：七行齐。
        assert_eq!(rows(&text).len(), 7, "{text}");
        let v: Value = serde_json::from_str(&raw_json).expect("json");
        assert_eq!(v["checks"][0]["extra"]["model_config_ok"], json!(false));
    }
}

/// 全跑那一档：第 ① 组和第 5 组**同时红，说的是同一件事的两面** —— 这是刻意留的重叠。
///
/// 第 ① 组问「这份配置起不起得来」（`--offline` 下照跑），第 5 组问「端点通不通」
/// （要联网）。更要紧的是第 5 组还是**兜底**：`config/aite.yaml` 不存在退到样例那一档，
/// 第 ① 组按下不表（样例里这两个字段天生是空的），那时唯一还会报它的就是第 5 组。
/// 谁哪天觉得「第 1 组已经报了、第 5 组降个级吧」，那一档就会静默放行 —— 这条守的是它。
#[tokio::test]
async fn the_fifth_row_still_reports_the_blank_model_section_on_a_full_run() {
    let root = tempfile::tempdir().expect("tempdir");
    let cfg = write_config_with_model(root.path(), "", DEFAULT_MODEL_NAME);
    let o = opts(root.path(), cfg, "http://127.0.0.1:1".to_string());

    let (report, text, _) = run(&o, &full_env()).await;

    assert_eq!(status_of(&report, "config"), Status::Fail, "{text}");
    assert_eq!(status_of(&report, "model"), Status::Fail, "{text}");
    for title in ["配置可加载", "模型端点通"] {
        assert!(
            row_for(&text, title).contains("model.base_url"),
            "{title} 这一行没说缺的是什么：{text}"
        );
    }
}

/// **两边口径对拍**：第 1 组因为 model 段拒绝的那份配置，真 `build_app` 也必须拒绝。
///
/// 口径照 [`build_app_really_refuses_what_the_first_check_refuses`]（注入那条）。这一条炸在
/// `build_app` 第 5 步（`build_model` → `OpenAiCompatModel::from_config`）—— 判据是
/// `model_blanks`，照抄的正是 `from_config` 那三条。哪天 `from_config` 改了口径而
/// `preflight.rs` 没跟上，这条会红。
#[tokio::test]
async fn build_app_really_refuses_a_blank_model_section() {
    let root = tempfile::tempdir().expect("tempdir");
    let prompt = root.path().join("prompts/platform.md");
    std::fs::create_dir_all(prompt.parent().expect("有父目录")).expect("建 prompts 目录");
    std::fs::write(&prompt, "# 假 system prompt\n").expect("写 prompt");
    let cfg_path = root.path().join("aite.yaml");
    // `build_app` 那一侧的 `storage.*` 是**裸相对路径**（相对进程 cwd），所以这一档全写绝对
    // 路径 —— 口径照 `write_config_absolute` 那段注释，别把 `data/` 建到 `core/crates/app/` 去。
    std::fs::write(
        &cfg_path,
        format!(
            "platform: feishu\nmodel:\n  provider: openai_compat\n  base_url: \"\"\n  \
             model: fake-model\nworker:\n  system_prompt_path: {}\nstorage:\n  \
             sqlite_path: {}\n  evidence_dir: {}\n  artifacts_dir: {}\nedge:\n  \
             edge_socket: run/nowhere-aite-edge.sock\n",
            prompt.display(),
            root.path().join("data/aite.db").display(),
            root.path().join("data/evidence").display(),
            root.path().join("data/artifacts").display(),
        ),
    )
    .expect("写 config");

    // preflight 这一侧：第 1 组 FAIL。
    let mut o = opts(
        root.path(),
        cfg_path.display().to_string(),
        "http://127.0.0.1:1".to_string(),
    );
    o.offline = true;
    let (report, text, _) = run(&o, &full_env()).await;
    assert_eq!(status_of(&report, "config"), Status::Fail, "{text}");

    // build_app 那一侧：同一份配置，必须拒绝起飞。
    let config = aite_app::load_config(&cfg_path).expect("配置本身是好的");
    let err = match aite_app::build_app(
        config,
        aite_app::Injections {
            repo_root: Some(root.path().to_path_buf()),
            ..aite_app::Injections::default()
        },
    )
    .await
    {
        Ok(_) => panic!("preflight 说这份 model 段起不来，build_app 却放行了"),
        Err(e) => e,
    };
    assert!(err.0.contains("base_url"), "两边说的不是同一件事：{err}");
}

// ===========================================================================
// 第 1 组的库判据（`storage.sqlite_path` 上那个文件当不当得了库）
// ===========================================================================

/// `sqlite_path` 指着一个不是 SQLite 的文件 → 第 1 组 FAIL，而第 7 组照样 OK。
///
/// **病史（这条测试守的就是它）**：2026-09-13，同一族的**第三个**口子。把 `sqlite_path`
/// 指到一个内容是 `this is definitely not a sqlite database` 的文件：
///
/// | | 结果 |
/// |---|---|
/// | `preflight --offline` | 全绿，「全部没红，可以起飞。」退出码 0 |
/// | `preflight` 全跑 | 第 1 组 OK、**第 7 组 OK**（红的那几组是本机没凭证 / 没起 edge，与它无关）|
/// | `aite run` | 退出码 2：`aite 起不来：建表失败（…）：sqlite: file is not a database` |
///
/// **第 7 组接不住它**：那一组问的是「三个路径的最近已存在祖先**目录**写得进去」，
/// 这一条问的是「那个**文件本身**能不能当库打开」。所以下面那句
/// `status_of(&report, "storage") == Ok` 不是顺手写的 —— 它就是「两件事」这个判断本身，
/// 谁哪天把库判据挪进第 7 组，这一行会红。
#[tokio::test]
async fn a_sqlite_path_that_is_not_a_database_fails_the_first_row() {
    let root = tempfile::tempdir().expect("tempdir");
    let db = root.path().join(DEFAULT_SQLITE_PATH);
    std::fs::create_dir_all(db.parent().expect("有父目录")).expect("建 data 目录");
    std::fs::write(&db, NOT_A_DATABASE).expect("写坏库");
    let stub = Stub::start(&happy_routes(FAKE_OPEN_ID)).await;
    let cfg = write_config_with_sqlite(
        root.path(),
        &format!("{}/v1", stub.base),
        DEFAULT_SQLITE_PATH,
    );

    let (report, text, raw_json) =
        run(&opts(root.path(), cfg, stub.base.clone()), &full_env()).await;

    let row = row_for(&text, "配置可加载");
    assert_eq!(status_of(&report, "config"), Status::Fail, "{text}");
    assert!(row.contains("storage.sqlite_path"), "{row}");
    assert!(
        !report.ok(),
        "第 1 组红了整份自检就该红（退出码 1）：{text}"
    );
    // 第 7 组照旧绿 —— 两件事，别混。
    assert_eq!(
        status_of(&report, "storage"),
        Status::Ok,
        "第 7 组不该管这一条，它问的是目录写不写得进去：{text}"
    );
    // 「怎么补」三件齐：当前值 / 该改成什么 / 不补的后果。
    let fix = report
        .checks
        .iter()
        .find(|c| c.name == "config")
        .expect("第 1 组")
        .fix
        .clone();
    assert!(fix.contains(&db.display().to_string()), "少了当前值：{fix}");
    assert!(
        fix.contains(&format!("里是 {DEFAULT_SQLITE_PATH}）")),
        "少了「该改成什么」（断到右括号，免得被 data/aite.db.bak 之类蒙过去）：{fix}"
    );
    assert!(fix.contains("建表失败"), "少了不补的后果：{fix}");
    // 一项失败不阻断后面的：七行齐，后面几组照跑。
    assert_eq!(rows(&text).len(), 7, "{text}");
    assert_eq!(status_of(&report, "model"), Status::Ok, "{text}");
    // --json 那一面也得说得出这件事。
    let v: Value = serde_json::from_str(&raw_json).expect("json");
    assert_eq!(
        v["checks"][0]["extra"]["sqlite_ok"],
        json!(false),
        "{raw_json}"
    );
    assert_eq!(
        v["checks"][0]["extra"]["sqlite_probed"],
        json!(true),
        "{raw_json}"
    );
}

/// 同一条判据在 `--offline` 下**照样跑** —— 这才是要紧的地方。
///
/// 上面那条的实证里，`--offline` 那一档是**全绿退出 0** 的：第 5/6 组那种「被 offline 跳过」
/// 的口子救不了它。谁哪天把库判据挪进「非 offline 才跑」的那半边，只有这条会红。
#[tokio::test]
async fn the_database_row_still_runs_offline() {
    let root = tempfile::tempdir().expect("tempdir");
    let db = root.path().join(DEFAULT_SQLITE_PATH);
    std::fs::create_dir_all(db.parent().expect("有父目录")).expect("建 data 目录");
    std::fs::write(&db, NOT_A_DATABASE).expect("写坏库");
    let cfg = write_config_with_sqlite(root.path(), "http://127.0.0.1:1/v1", DEFAULT_SQLITE_PATH);
    let mut o = opts(root.path(), cfg, "http://127.0.0.1:1".to_string());
    o.offline = true;

    let (report, text, _) = run(&o, &full_env()).await;

    assert_eq!(status_of(&report, "config"), Status::Fail, "{text}");
    assert!(
        row_for(&text, "配置可加载").contains("storage.sqlite_path"),
        "{text}"
    );
    assert!(!report.ok(), "{text}");
}

/// 好库不许误伤，**库还不在**也不许误伤 —— 而且第 1 组一个字节都不许落盘。
///
/// 后半句是选「折进第 ① 组」而不是「扩第 7 组」时立下的准入条件：`SqliteSessionStore::open`
/// 会 `create_dir_all` 父目录并新建库文件，判据要是不先问「文件在不在」，光跑一次
/// `aite preflight` 就能把 `data/aite.db` 建出来 —— 而「配置可加载」这一组凭什么建东西。
/// （`src/preflight.rs` 的 `the_db_probe_never_writes_anything` 在纯函数那一层钉同一件事；
/// 这条是端到端那一层，连第 7 组那个「真建一个目录再删」的探针一起算进来。）
#[tokio::test]
async fn a_healthy_or_absent_database_does_not_trip_the_first_row() {
    // 1) 真库：`store.init()` 建完表的那种。
    let root = tempfile::tempdir().expect("tempdir");
    let db = root.path().join(DEFAULT_SQLITE_PATH);
    std::fs::create_dir_all(db.parent().expect("有父目录")).expect("建 data 目录");
    let store = SqliteSessionStore::open(&db).expect("建库");
    store.init().await.expect("建表");
    store.close().await.expect("关库");
    let cfg = write_config_with_sqlite(root.path(), "http://127.0.0.1:1/v1", DEFAULT_SQLITE_PATH);
    let mut o = opts(root.path(), cfg, "http://127.0.0.1:1".to_string());
    o.offline = true;

    let (report, text, _) = run(&o, &full_env()).await;

    assert_eq!(
        status_of(&report, "config"),
        Status::Ok,
        "好库被误伤了：{text}"
    );

    // 2) 库还不在：照样 OK，而且跑完之后它**仍然**不在（起飞时才建，见 `build_app` 注释）。
    let fresh = tempfile::tempdir().expect("tempdir");
    let cfg = write_config_with_sqlite(fresh.path(), "http://127.0.0.1:1/v1", DEFAULT_SQLITE_PATH);
    let mut o = opts(fresh.path(), cfg, "http://127.0.0.1:1".to_string());
    o.offline = true;

    let (report, text, _) = run(&o, &full_env()).await;

    assert_eq!(status_of(&report, "config"), Status::Ok, "{text}");
    assert!(
        !fresh.path().join(DEFAULT_SQLITE_PATH).exists(),
        "第 1 组把库文件建出来了 —— 它是「配置可加载」，不该有落盘副作用"
    );
    assert!(
        !fresh.path().join("data").exists(),
        "连 data/ 都建出来了：{}",
        fresh.path().display()
    );
}

/// **两边口径对拍**：第 1 组拒绝的那份库，起飞路径上那一步真的也炸。
///
/// 口径照 `build_app_really_refuses_what_the_first_check_refuses`（注入那条），但对的**不是**
/// `build_app` —— 这一条炸在 `run.rs` 的 `takeoff` 里那句 `app.store.init()`。
/// `SqliteSessionStore::open`（`app.rs` 第 6 步）只是 `Connection::open`，SQLite 在那一步
/// 根本不读文件头，所以组装那一关是过得去的：**这里对拍 `open` 会得到一个恒真的绿**。
/// 判据因此照着真正会炸的那一步来（逼它读一次文件头），这条测试就是那个对拍。
#[tokio::test]
async fn store_init_really_fails_on_what_the_first_check_refuses() {
    let root = tempfile::tempdir().expect("tempdir");
    let db = root.path().join(DEFAULT_SQLITE_PATH);
    std::fs::create_dir_all(db.parent().expect("有父目录")).expect("建 data 目录");
    std::fs::write(&db, NOT_A_DATABASE).expect("写坏库");
    let cfg = write_config_with_sqlite(root.path(), "http://127.0.0.1:1/v1", DEFAULT_SQLITE_PATH);
    let mut o = opts(root.path(), cfg, "http://127.0.0.1:1".to_string());
    o.offline = true;

    // preflight 这一侧：第 1 组 FAIL。
    let (report, text, _) = run(&o, &full_env()).await;
    assert_eq!(status_of(&report, "config"), Status::Fail, "{text}");

    // 起飞那一侧：`open` 照样是 Ok（这正是判据不能只 open 的理由），`init()` 才炸。
    let store = SqliteSessionStore::open(&db).expect("open 这一步 SQLite 不读文件头，必过");
    let err = store
        .init()
        .await
        .expect_err("preflight 说这个库起不来，store.init() 却建表成功了");
    assert!(
        err.to_string().contains("not a database"),
        "两边说的不是同一件事：{err}"
    );
}

/// **好库但只读** → 第 1 组 FAIL，第 7 组照样 OK，`store.init()` 那一侧真的也炸。
///
/// **病史（这条测试守的就是它）**：Z1 补完「不是个库」那一半之后，`sqlite_fault` 的文档
/// 注释里明写着「探不出来的那一半」：文件是个好库、但它自己只读时 `CREATE TABLE` 照样炸。
/// Z1 / Z3 / AA4 记了三轮没治，理由是「要覆盖它得真往库里写一次，把第 ① 组从纯读变成有
/// 副作用，不划算」。**那个前提不成立**：不用写 —— 要一个写句柄就够了，一个字节不落。
///
/// 2026-09-15 实测的改前现场（`chmod 444` 的合法库）：
///
/// | | 结果 |
/// |---|---|
/// | `preflight --offline` | 全绿，「全部没红，可以起飞。」退出码 0 |
/// | `preflight` 全跑 | 第 1 组 OK、**第 7 组 OK**（红的那几组是本机没凭证 / 没起 edge，与它无关）|
/// | `aite run` | 退出码 2：`aite 起不来：建表失败（…）：sqlite: attempt to write a readonly database` |
///
/// 下面那句 `status_of(&report, "storage") == Ok` 和库那条一样不是顺手写的 —— 第 7 组问的是
/// 「三个路径的最近已存在祖先**目录**写得进去」（`CREATE TABLE` 还要在父目录里建 journal，
/// 归它管），这里问的是「这个**文件本身**我写不写得动」。父目录可写而文件 444 时只有第 ① 组看得见。
///
/// **以 root 跑这一条会红** —— 那不是 bug，root 下 `CREATE TABLE` 本来也写得进去。
///
/// **判据为什么挂在「文件写不写得动」而不是「`init()` 会不会炸」**：库**已经建完表**时，
/// `init()` 的 `CREATE TABLE IF NOT EXISTS` 在只读库上是个**空操作、会成功**
/// （实测：`sqlite3 ro.db "CREATE TABLE IF NOT EXISTS t(x)"` 退出 0，
/// 同一个库上 `INSERT` 报 `attempt to write a readonly database`）。
/// 也就是说只读库有两种死法：还没建表 → 起飞时死在 `init()`（下面对拍的就是这一种）；
/// 已经建完表 → 起飞成功，**死在群里第一个任务落库那一下**，比前一种更难查。
/// 两种都是死，所以 preflight 拦的是文件本身 —— 拿 `init()` 当判据只能拦住一半。
#[tokio::test]
async fn a_readonly_database_fails_the_first_row_and_still_leaves_the_seventh_green() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().expect("tempdir");
    let db = root.path().join(DEFAULT_SQLITE_PATH);
    std::fs::create_dir_all(db.parent().expect("有父目录")).expect("建 data 目录");
    // 0 字节的文件在 SQLite 眼里是一个**合法的空库**（`src/preflight.rs` 的
    // `the_db_probe_never_writes_anything` 钉着这条），也正是 `build_app` 第 6 步
    // `SqliteSessionStore::open` 给还不存在的库留下的那个形状。所以内容那一半是好的 ——
    // 红的只可能是权限，否则这条就变成 Z1 那条判据的复读。
    std::fs::write(&db, b"").expect("建空库");
    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o444)).expect("改只读");
    let cfg = write_config_with_sqlite(root.path(), "http://127.0.0.1:1/v1", DEFAULT_SQLITE_PATH);
    let mut o = opts(root.path(), cfg, "http://127.0.0.1:1".to_string());
    o.offline = true;

    let (report, text, raw_json) = run(&o, &full_env()).await;

    assert_eq!(status_of(&report, "config"), Status::Fail, "{text}");
    let row = row_for(&text, "配置可加载");
    assert!(row.contains("storage.sqlite_path"), "{row}");
    assert!(
        row.contains("写不进去"),
        "报错报成「当不了库」了 —— 内容是好的，卡的是权限：{row}"
    );
    assert!(!report.ok(), "{text}");
    // 第 7 组照旧绿 —— 两件事，别混。
    assert_eq!(
        status_of(&report, "storage"),
        Status::Ok,
        "第 7 组不该管这一条，它问的是目录写不写得进去：{text}"
    );
    // 「怎么补」三件齐：当前值 / 怎么改 / 不补的后果（`store.init()` 的原话）。
    let fix = report
        .checks
        .iter()
        .find(|c| c.name == "config")
        .expect("第 1 组")
        .fix
        .clone();
    assert!(fix.contains(&db.display().to_string()), "少了当前值：{fix}");
    assert!(fix.contains("chmod u+w"), "少了「怎么改」：{fix}");
    assert!(
        fix.contains("attempt to write a readonly database"),
        "少了不补的后果：{fix}"
    );
    assert_eq!(rows(&text).len(), 7, "{text}");
    let v: Value = serde_json::from_str(&raw_json).expect("json");
    assert_eq!(v["checks"][0]["extra"]["sqlite_ok"], json!(false));

    // **两边口径对拍**：`open` 照样是 Ok（SQLite 到这一步还不写），`init()` 才炸。
    let store = SqliteSessionStore::open(&db).expect("open 这一步 SQLite 还不写，必过");
    let err = store
        .init()
        .await
        .expect_err("preflight 说这个库写不进去，store.init() 却建表成功了");
    assert!(
        err.to_string().contains("readonly"),
        "两边说的不是同一件事：{err}"
    );

    // 收尾：把写权限加回去，免得 tempdir 清理在别的平台上卡住。
    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).expect("改回可写");
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
/// 没飞书凭证的机器上 `--offline` 要能跑 —— 缺变量降成 WARN（不拦起飞），但一个名字都不少报。
///
/// **2026-09-15（BB6）把「没凭证」拆成了两半，因为它们对起飞的后果根本不同**：
///
/// * **飞书那三个变量是 edge（Go）读的**，`build_app` 从头到尾不碰它们 —— 缺了照样起得来
///   （实测：三个全不设，`aite run` 一路走到 `serve()`）。所以第 2 组那条 offline 降级
///   对它们是对的，这条测试的前半段原样守着。
/// * **`model.api_key_env` 点到的那个是 core 自己读的**：`build_app` 第 5 步
///   `build_model` → `from_config` → `resolve_api_key`，缺了当场退出码 2
///   （`模型配置不完整：环境变量 AITE_MODEL_API_KEY 没设置或为空`）。它在第 2 组里
///   **只是 WARN**，于是 2026-09-15 之前 `--offline` 在这种机器上报「全部没红，可以起飞」
///   而 `aite run` 退出码 2 —— 与 X1 / Y2 / Z1 那三条同族，实测在案。现在归第 ① 组。
///
/// 所以「没凭证还能跑」这句话现在的准确说法是：**没飞书凭证能跑，没模型密钥不能**。
#[tokio::test]
async fn offline_without_credentials_still_passes() {
    let root = tempfile::tempdir().expect("tempdir");
    let stub = Stub::start(&[]).await;
    let cfg = write_config(root.path(), &format!("{}/v1", stub.base));
    let mut o = opts(root.path(), cfg, stub.base.clone());
    o.offline = true;

    // 1) 飞书三项全缺、模型密钥在：照旧全绿，第 2 组 WARN 且四个名字一个不少。
    let only_model_key =
        HashMap::from([("AITE_MODEL_API_KEY".to_string(), FAKE_MODEL_KEY.to_string())]);
    let (report, text, _) = run(&o, &only_model_key).await;

    assert!(report.ok(), "没飞书凭证不该拦住起飞：{text}");
    assert_eq!(status_of(&report, "env"), Status::Warn, "{text}");
    let env_row = row_for(&text, "环境变量齐");
    for name in full_env().keys() {
        assert!(env_row.contains(name.as_str()), "少报了 {name}：{env_row}");
    }

    // 2) 连模型密钥也没有：第 ① 组 FAIL —— 这一发 `aite run` 起不来，`--offline` 不许说
    //    「可以起飞」。第 2 组那条降级**不许跟着变**，它管的是另一件事。
    let (report, text, _) = run(&o, &HashMap::new()).await;

    assert!(
        !report.ok(),
        "没有模型密钥 aite run 会退出码 2，--offline 不该全绿：{text}"
    );
    assert_eq!(status_of(&report, "config"), Status::Fail, "{text}");
    assert!(
        row_for(&text, "配置可加载").contains("AITE_MODEL_API_KEY"),
        "第 ① 组要点名缺的是哪一个：{text}"
    );
    assert_eq!(
        status_of(&report, "env"),
        Status::Warn,
        "第 2 组那条 offline 降级是给飞书三项的，不该被这一轨改掉：{text}"
    );
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
    // `base_url` / `model` 同理（2026-09-15 起第 1 组也验这两个）—— 契约默认值是空串。
    let prompt = write_prompt(root.path());
    std::fs::write(
        &path,
        format!(
            "model:\n  api_key_env: {custom}\n  base_url: http://127.0.0.1:1/v1\n  \
             model: fake-model\nworker:\n  system_prompt_path: {prompt}\n\
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
