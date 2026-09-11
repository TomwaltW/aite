//! FakeToolGateway —— `ToolGateway` 的替身（对应旧 `aite/testing/fake_gateway.py`）。
//!
//! 真实现归 R6；这一份是给评测 runner 与各轨测试用的，好让 R6 还没合进来时场景仍然
//! 能跑出「工具被调过、返回了什么」。
//!
//! 严格按契约注释的顺序办事：
//!     校验 session_token → 查工具存在 → 按 ToolSpec.parameters 校验 arguments
//!     → 执行（带超时）→ 截断 content → 返回
//! 并且**永远不把错误抛给调用方**，所有失败都走 `ToolResult{ok:false, error}`。
//!
//! `read_group_history` 在这里过滤掉非真人消息，`PlatformPort::read_history` 则不过滤
//! —— 05_history_summary 验的就是这道分工。
//!
//! ⚠ 各工具的 content 排版**与真实现有意不同**（清单 §5）：场景断言耦合的是这一套，
//! 必须逐字保留。
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aite_contracts::{
    DEFAULT_TOOL_TIMEOUT_SEC, ExecRequest, MAX_TOOL_CONTENT_CHARS, PlatformPort, SandboxPort,
    SandboxSpec, ToolCallRequest, ToolContext, ToolError, ToolErrorCode, ToolGateway, ToolResult,
    ToolSpec, gateway_tools,
};
use async_trait::async_trait;
use serde_json::{Map, Value, json};

use crate::fake_platform::FakePlatform;
use crate::fake_sandbox::FakeSandbox;
use crate::jsonschema_mini::validate_arguments;
use crate::kwargs;
use crate::recorder::CallLog;

fn spec_of(name: &str) -> Option<&'static ToolSpec> {
    gateway_tools().iter().find(|t| t.name == name)
}

fn tool_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = gateway_tools().iter().map(|t| t.name.as_str()).collect();
    names.sort_unstable();
    names
}

/// `ToolGateway` 的替身，背后接 FakePlatform + FakeSandbox。
pub struct FakeToolGateway {
    pub platform: Arc<FakePlatform>,
    pub sandbox: Arc<FakeSandbox>,
    /// None = 不校验（大多数场景不关心）；给了就必须与 `ctx.session_token` 一致
    pub expected_token: Option<String>,
    pub timeout_sec: u64,
    pub sandbox_image: String,
    pub calls: CallLog,
    results: Mutex<Vec<ToolResult>>,
    sandbox_of: Mutex<BTreeMap<String, String>>,
    tokens: Mutex<BTreeMap<String, String>>,
}

impl FakeToolGateway {
    pub fn new(platform: Arc<FakePlatform>, sandbox: Arc<FakeSandbox>) -> Self {
        Self {
            platform,
            sandbox,
            expected_token: None,
            timeout_sec: DEFAULT_TOOL_TIMEOUT_SEC,
            sandbox_image: "aite-sandbox:p0".to_string(),
            calls: CallLog::new(),
            results: Mutex::new(Vec::new()),
            sandbox_of: Mutex::new(BTreeMap::new()),
            tokens: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn with_expected_token(mut self, token: &str) -> Self {
        self.expected_token = Some(token.to_string());
        self
    }

    pub fn with_timeout_sec(mut self, secs: u64) -> Self {
        self.timeout_sec = secs;
        self
    }

    pub fn with_sandbox_image(mut self, image: &str) -> Self {
        self.sandbox_image = image.to_string();
        self
    }

    // ---- 给断言用 --------------------------------------------------------

    pub fn count(&self, name: &str) -> usize {
        self.calls
            .of("call")
            .iter()
            .filter(|c| c.arg_str("name") == Some(name))
            .count()
    }

    pub fn results(&self) -> Vec<ToolResult> {
        self.results.lock().expect("FakeGateway 锁").clone()
    }

    pub fn results_of(&self, name: &str) -> Vec<ToolResult> {
        self.results()
            .into_iter()
            .filter(|r| r.name == name)
            .collect()
    }

    // ---- 内部 -----------------------------------------------------------

    fn done(
        &self,
        idx: usize,
        req: &ToolCallRequest,
        started: std::time::Instant,
        outcome: Outcome,
    ) -> ToolResult {
        let Outcome {
            content,
            data,
            error,
        } = outcome;
        let ok = error.is_none();
        let tag = match &error {
            None => format!("ok={ok}"),
            Some(e) => format!("ok={ok} {}", e.code.as_str()),
        };
        let result = ToolResult {
            call_id: req.call_id.clone(),
            name: req.name.clone(),
            ok,
            // 替身这一档是硬截断、不带标记（真实现才加 `[内容已截断]`）
            content: content.chars().take(MAX_TOOL_CONTENT_CHARS).collect(),
            data,
            error,
            duration_ms: started.elapsed().as_millis() as u64,
            artifacts: Vec::new(),
        };
        self.calls.set_result(idx, json!(tag));
        self.results
            .lock()
            .expect("FakeGateway 锁")
            .push(result.clone());
        result
    }

    /// 每个 task 一个沙箱，用到才建。
    async fn sandbox_for(&self, ctx: &ToolContext) -> Result<String, String> {
        if let Some(id) = self
            .sandbox_of
            .lock()
            .expect("FakeGateway 锁")
            .get(&ctx.task_id)
        {
            return Ok(id.clone());
        }
        let spec = SandboxSpec::new(self.sandbox_image.clone());
        let id = self
            .sandbox
            .acquire(&ctx.task_id, &spec)
            .await
            .map_err(|e| e.to_string())?;
        self.sandbox_of
            .lock()
            .expect("FakeGateway 锁")
            .insert(ctx.task_id.clone(), id.clone());
        Ok(id)
    }

    async fn dispatch(
        &self,
        ctx: &ToolContext,
        name: &str,
        args: &Map<String, Value>,
    ) -> Result<(String, Map<String, Value>), Failure> {
        match name {
            "read_group_history" => self.tool_read_group_history(ctx, args).await,
            "read_document" => self.tool_read_document(args).await,
            "download_attachment" => self.tool_download_attachment(ctx, args).await,
            "run_python" => self.tool_run_python(ctx, args).await,
            "list_files" => self.tool_list_files(ctx).await,
            other => Err(Failure::upstream(format!("没有这个工具实现：{other}"))),
        }
    }

    async fn tool_read_group_history(
        &self,
        ctx: &ToolContext,
        args: &Map<String, Value>,
    ) -> Result<(String, Map<String, Value>), Failure> {
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50) as u32;
        let thread_only = args
            .get("thread_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let thread_id = if thread_only {
            ctx.thread_id.as_deref()
        } else {
            None
        };
        let rows = self
            .platform
            .read_history(&ctx.chat_id, limit, thread_id)
            .await
            .map_err(|e| Failure::upstream(e.message))?;
        // 过滤归 Gateway，不归 adapter
        let human: Vec<_> = rows.iter().filter(|h| h.sender_kind == "human").collect();
        let lines: Vec<String> = human
            .iter()
            .map(|h| {
                let who = h.sender_name.clone().unwrap_or_else(|| h.sender_id.clone());
                format!("[{}] {}: {}", h.message_id, who, h.text)
            })
            .collect();
        Ok((
            lines.join("\n"),
            kwargs! {
                "count" => json!(human.len()),
                "dropped" => json!(rows.len() - human.len()),
            },
        ))
    }

    async fn tool_read_document(
        &self,
        args: &Map<String, Value>,
    ) -> Result<(String, Map<String, Value>), Failure> {
        let key = args
            .get("url_or_token")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let doc = self
            .platform
            .read_document(key)
            .await
            .map_err(|e| Failure::upstream(e.message))?;
        Ok((
            format!("# {}\n\n{}", doc.title, doc.text),
            kwargs! {"title" => json!(doc.title), "url" => json!(doc.url)},
        ))
    }

    async fn tool_download_attachment(
        &self,
        ctx: &ToolContext,
        args: &Map<String, Value>,
    ) -> Result<(String, Map<String, Value>), Failure> {
        let Some(message_id) = ctx
            .attachments_message_id
            .as_deref()
            .filter(|s| !s.is_empty())
        else {
            return Err(Failure::upstream(
                "本次消息没有附件（ToolContext.attachments_message_id 为空）".to_string(),
            ));
        };
        let file_key = args
            .get("file_key")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let data = self
            .platform
            .download_file(message_id, file_key)
            .await
            .map_err(|e| Failure::upstream(e.message))?;
        let sandbox_id = self.sandbox_for(ctx).await.map_err(Failure::sandbox)?;
        // ⚠ 替身用**未清洗**的 file_key（真实现会走 _safe_name），场景断言耦合这一套
        let path = format!("/work/in/{file_key}");
        self.sandbox
            .put_file(&sandbox_id, &path, &data)
            .await
            .map_err(|e| Failure::sandbox(e.message))?;
        Ok((
            format!("已下载到 {path}（{} 字节）", data.len()),
            kwargs! {"path" => json!(path), "size" => json!(data.len())},
        ))
    }

    async fn tool_run_python(
        &self,
        ctx: &ToolContext,
        args: &Map<String, Value>,
    ) -> Result<(String, Map<String, Value>), Failure> {
        let sandbox_id = self.sandbox_for(ctx).await.map_err(Failure::sandbox)?;
        let code = args.get("code").and_then(Value::as_str).unwrap_or_default();
        let timeout_sec = args
            .get("timeout_sec")
            .and_then(Value::as_u64)
            .unwrap_or(120) as u32;
        let res = self
            .sandbox
            .exec(&sandbox_id, &ExecRequest::python(code, timeout_sec))
            .await
            .map_err(|e| Failure::sandbox(e.message))?;
        let mut body = format!(
            "exit_code={}\n--- stdout ---\n{}",
            res.exit_code, res.stdout
        );
        if !res.stderr.is_empty() {
            body.push_str(&format!("\n--- stderr ---\n{}", res.stderr));
        }
        Ok((
            body,
            kwargs! {
                "exit_code" => json!(res.exit_code),
                "files_out" => json!(res.files_out.iter().map(|f| f.path.clone()).collect::<Vec<_>>()),
                "duration_ms" => json!(res.duration_ms),
            },
        ))
    }

    async fn tool_list_files(
        &self,
        ctx: &ToolContext,
    ) -> Result<(String, Map<String, Value>), Failure> {
        let sandbox_id = self.sandbox_for(ctx).await.map_err(Failure::sandbox)?;
        let files = self
            .sandbox
            .list_files(&sandbox_id)
            .await
            .map_err(|e| Failure::sandbox(e.message))?;
        let content = if files.is_empty() {
            "(空)".to_string()
        } else {
            files.join("\n")
        };
        Ok((content, kwargs! {"files" => json!(files)}))
    }
}

/// 工具执行里的失败：带一个错误码。
struct Failure {
    code: ToolErrorCode,
    message: String,
}

impl Failure {
    fn upstream(message: String) -> Self {
        Self {
            code: ToolErrorCode::Upstream,
            message,
        }
    }
    fn sandbox(message: String) -> Self {
        Self {
            code: ToolErrorCode::Sandbox,
            message,
        }
    }
}

/// 一次调用的结局：成功的 content + data，或一个 ToolError。
struct Outcome {
    content: String,
    data: Option<Map<String, Value>>,
    error: Option<ToolError>,
}

impl Outcome {
    fn ok(content: String, data: Map<String, Value>) -> Self {
        Self {
            content,
            data: Some(data),
            error: None,
        }
    }

    /// 失败：message 同时写进 content（给模型看）和 error.message。
    fn failed(code: ToolErrorCode, content: String, message: String) -> Self {
        Self {
            content,
            data: None,
            error: Some(ToolError { code, message }),
        }
    }
}

#[async_trait]
impl ToolGateway for FakeToolGateway {
    fn catalog(&self, ctx: &ToolContext) -> Vec<ToolSpec> {
        self.calls
            .record("catalog", kwargs! {"task_id" => json!(ctx.task_id)});
        gateway_tools().to_vec()
    }

    async fn call(&self, ctx: &ToolContext, req: &ToolCallRequest) -> ToolResult {
        let idx = self.calls.record(
            "call",
            kwargs! {
                "name" => json!(req.name),
                "task_id" => json!(ctx.task_id),
                "arguments" => Value::Object(req.arguments.clone()),
            },
        );
        let started = std::time::Instant::now();

        // 1. token
        let expected = self.expected_token.clone().or_else(|| {
            self.tokens
                .lock()
                .expect("FakeGateway 锁")
                .get(&ctx.task_id)
                .cloned()
        });
        if let Some(expected) = expected
            && ctx.session_token != expected
        {
            return self.done(
                idx,
                req,
                started,
                Outcome::failed(
                    ToolErrorCode::Denied,
                    "会话令牌不匹配".to_string(),
                    "session_token 不匹配".to_string(),
                ),
            );
        }

        // 2. 工具存在
        let Some(spec) = spec_of(&req.name) else {
            let names = tool_names();
            return self.done(
                idx,
                req,
                started,
                Outcome::failed(
                    ToolErrorCode::NotFound,
                    format!("没有名为 {} 的工具", req.name),
                    format!("未知工具 {}，可用：{names:?}", req.name),
                ),
            );
        };

        // 3. schema（补 default）
        let args = match validate_arguments(&req.arguments, &spec.parameters) {
            Ok(args) => args,
            Err(e) => {
                return self.done(
                    idx,
                    req,
                    started,
                    Outcome::failed(ToolErrorCode::InvalidArgs, e.0.clone(), e.0),
                );
            }
        };

        // 4. 执行（带超时）
        let budget = if req.name == "run_python" {
            args.get("timeout_sec")
                .and_then(Value::as_u64)
                .unwrap_or(self.timeout_sec)
        } else {
            self.timeout_sec
        };
        let outcome = tokio::time::timeout(
            Duration::from_secs(budget),
            self.dispatch(ctx, &req.name, &args),
        )
        .await;

        let outcome = match outcome {
            Err(_) => Outcome::failed(
                ToolErrorCode::Timeout,
                format!("{} 执行超时（{budget}s）", req.name),
                "timeout".to_string(),
            ),
            Ok(Err(failure)) => {
                Outcome::failed(failure.code, failure.message.clone(), failure.message)
            }
            Ok(Ok((content, data))) => Outcome::ok(content, data),
        };
        self.done(idx, req, started, outcome)
    }

    fn register_task(&self, task_id: &str, session_token: &str) {
        self.tokens
            .lock()
            .expect("FakeGateway 锁")
            .insert(task_id.to_string(), session_token.to_string());
    }

    fn unregister_task(&self, task_id: &str) {
        self.tokens.lock().expect("FakeGateway 锁").remove(task_id);
    }

    /// 这个 task 现在用的沙箱（还没建就是 None）。
    ///
    /// 冻结的 ToolGateway 协议里沙箱归属曾经不在协议内 —— 但产物是在这里建的沙箱里写
    /// 出来的，worker 取产物、控制面 `!stop` 都得问得到它。替身缺了就不等价：worker 会
    /// 以为没有沙箱、自己 acquire 一个空的，04_csv_to_chart 的产物就此丢掉。
    async fn sandbox_id_of(&self, task_id: &str) -> Option<String> {
        self.sandbox_of
            .lock()
            .expect("FakeGateway 锁")
            .get(task_id)
            .cloned()
    }

    /// 释放这个 task 的沙箱（幂等）。
    async fn release_task(&self, task_id: &str) {
        self.tokens.lock().expect("FakeGateway 锁").remove(task_id);
        let sandbox_id = self
            .sandbox_of
            .lock()
            .expect("FakeGateway 锁")
            .remove(task_id);
        if let Some(id) = sandbox_id {
            let _ = self.sandbox.release(&id).await;
        }
    }
}
