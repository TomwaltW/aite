//! `P0ToolGateway`（对应旧 `aite/gateway/tool_gateway.py`）。
//!
//! `call` 的顺序照 §3.2 的注释逐字执行：
//!
//! ```text
//! 校验 session_token → 查工具存在 → 按 ToolSpec.parameters 校验 arguments
//! → 执行（带超时）→ 截断 content → 返回
//! ```
//!
//! 以及那条硬要求：**永远不往外抛**。`call` 的返回类型就是 `ToolResult`，工具实现 panic
//! 也被接住翻成 `upstream`（不让一个写崩的工具带走整个 worker）。唯一"没有结果"的情形是
//! 调用方把这个 future 丢掉（取消）—— 那是调用方自己的意思，不该伪造出一条 ToolResult。
//!
//! session_token 的校验是**失败关闭**的：查不到这个 task_id 该有的 token 就一律 denied。
//! Gateway 不认识 SessionStore，所以期望值有两个来源：`register_task()` 现场登记，
//! 或者构造时传一个 `token_resolver`（RΩ 组装时把它接到 `SessionStore` 上）。
//! 两个都没有 = 谁也别想调工具，而不是「没登记就放过」。
use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use aite_contracts::ports::BoxFuture;
use aite_contracts::{
    DEFAULT_TOOL_TIMEOUT_SEC, MAX_TOOL_CONTENT_CHARS, PlatformPort, SandboxPort, SandboxSpec,
    ToolCallRequest, ToolContext, ToolError, ToolErrorCode, ToolGateway, ToolResult, ToolSpec,
    gateway_tools,
};
use async_trait::async_trait;
use futures::FutureExt;
use serde_json::{Map, Value};
use tokio::time::Instant;

use crate::schema::validate_arguments;
use crate::tools::{ToolEnv, ToolFailure, ToolImpl, ToolOutcome, default_tools};

/// task_id → 期望的 session_token。取不到（store 抖动等）→ `Err(人话)`，Gateway 翻成 upstream。
pub type TokenResolver = Arc<dyn Fn(&str) -> Result<Option<String>, String> + Send + Sync>;

/// `run_python` 的外层超时比它自己的 timeout_sec 多留一点余量。
///
/// 代码的时限由容器内的 coreutils `timeout` 精确执行（超时退出码 124，工具翻成 timeout）；
/// 外层这道 `timeout` 只在 Docker daemon 卡住、连 exec 都不返回时才该开火。
/// 余量为 0 的话两个时限同时到，赢的通常是外层，模型就永远拿不到超时前的部分输出。
pub const DEFAULT_RUN_PYTHON_GRACE_SEC: f64 = 5.0;

/// 工具能碰到的两个外部面 + 沙箱记账。`ToolEnv` 拿着它的 `Arc`。
pub(crate) struct Inner {
    platform: Option<Arc<dyn PlatformPort>>,
    sandbox: Option<Arc<dyn SandboxPort>>,
    spec: SandboxSpec,
    /// task_id → sandbox_id（沙箱归 Gateway 记账，worker 取产物前先问 `sandbox_id_of`）
    sandbox_ids: Mutex<HashMap<String, String>>,
    /// task_id → 建沙箱的串行锁（并发两个 run_python 不会各建一个）
    acquire_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Inner {
    pub(crate) fn platform(&self) -> Option<Arc<dyn PlatformPort>> {
        self.platform.clone()
    }

    pub(crate) fn sandbox(&self) -> Option<Arc<dyn SandboxPort>> {
        self.sandbox.clone()
    }

    pub(crate) fn current_sandbox_id(&self, task_id: &str) -> Option<String> {
        self.sandbox_ids
            .lock()
            .expect("sandbox_ids 锁")
            .get(task_id)
            .cloned()
    }

    /// 容器在 edge 那边已经没了时，把记账里那一条摘掉 —— 下一次 `acquire_sandbox`
    /// 就会老老实实建个新的，而不是把死 id 再复用出来。
    ///
    /// 真会走到这里的是 W7 的 reaper：它按 `sandbox.idle_sec`（默认 300s）收空闲容器，
    /// 而 `touch` 只在沙箱 RPC 时发生 —— 模型在两次工具调用之间想上五分钟，中间一次都不刷。
    /// edge 那边 `ReapIdle` 把收走的 id 交回控制面，但控制面只拿它计数（`plane.rs` 的
    /// `reaper_loop`），**Gateway 这张表没有人通知**。所以只能由用到它的那一侧自愈。
    ///
    /// 只动记账，不发 `release`：那个 id 在 edge 那边已经不存在，再 release 一次
    /// 换回来的还是一条 NotFound。
    pub(crate) fn forget_sandbox(&self, task_id: &str) -> Option<String> {
        self.sandbox_ids
            .lock()
            .expect("sandbox_ids 锁")
            .remove(task_id)
    }

    /// 一个 task 一个沙箱，有就复用。
    pub(crate) async fn acquire_sandbox(&self, task_id: &str) -> Result<String, ToolFailure> {
        if let Some(existing) = self.current_sandbox_id(task_id) {
            return Ok(existing);
        }
        let Some(sandbox) = self.sandbox() else {
            return Err(ToolFailure::sandbox("本次运行没有可用沙箱"));
        };

        let lock = {
            let mut locks = self.acquire_locks.lock().expect("acquire_locks 锁");
            locks
                .entry(task_id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;
        // 双检：等锁的这段时间里别人可能已经建好了。
        if let Some(existing) = self.current_sandbox_id(task_id) {
            return Ok(existing);
        }
        let sandbox_id = sandbox
            .acquire(task_id, &self.spec)
            .await
            .map_err(|e| ToolFailure::sandbox(e.to_string()))?;
        self.sandbox_ids
            .lock()
            .expect("sandbox_ids 锁")
            .insert(task_id.to_string(), sandbox_id.clone());
        Ok(sandbox_id)
    }
}

/// ToolGateway 的 P0 实现。catalog 就是 `gateway_tools()` 原样。
pub struct P0ToolGateway {
    inner: Arc<Inner>,
    specs: Vec<ToolSpec>,
    impls: HashMap<String, ToolImpl>,
    tokens: Mutex<HashMap<String, String>>,
    token_resolver: Option<TokenResolver>,
    default_timeout_sec: f64,
    run_python_grace_sec: f64,
}

impl P0ToolGateway {
    /// RΩ 组装用的入口（§3.2 的构造表）。
    pub fn new(
        platform: Arc<dyn PlatformPort>,
        sandbox: Arc<dyn SandboxPort>,
        spec: SandboxSpec,
    ) -> Self {
        Self::with_optional_ports(Some(platform), Some(sandbox), spec)
    }

    /// 缺面也要能起：没接平台时 `read_*` 返回 upstream、没接沙箱时 `run_python` 返回 sandbox
    /// （旧实现靠 `platform=None` / `sandbox=None` 表达，Rust 里显式化成这个构造器）。
    pub fn with_optional_ports(
        platform: Option<Arc<dyn PlatformPort>>,
        sandbox: Option<Arc<dyn SandboxPort>>,
        spec: SandboxSpec,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                platform,
                sandbox,
                spec,
                sandbox_ids: Mutex::new(HashMap::new()),
                acquire_locks: Mutex::new(HashMap::new()),
            }),
            specs: gateway_tools().to_vec(),
            impls: default_tools(),
            tokens: Mutex::new(HashMap::new()),
            token_resolver: None,
            default_timeout_sec: DEFAULT_TOOL_TIMEOUT_SEC as f64,
            run_python_grace_sec: DEFAULT_RUN_PYTHON_GRACE_SEC,
        }
    }

    /// 非 `run_python` 工具的超时预算（秒）。
    pub fn with_default_timeout(mut self, seconds: f64) -> Self {
        self.default_timeout_sec = seconds;
        self
    }

    /// `run_python` 外层预算相对 `timeout_sec` 的余量（秒）。
    pub fn with_run_python_grace(mut self, seconds: f64) -> Self {
        self.run_python_grace_sec = seconds;
        self
    }

    /// 期望 token 的来源换成外部查询（RΩ 接到 `SessionStore` 上）。给了它就不看登记表。
    pub fn with_token_resolver(mut self, resolver: TokenResolver) -> Self {
        self.token_resolver = Some(resolver);
        self
    }

    /// 换掉某个工具的实现（测试用；键必须是 `gateway_tools()` 里的名字）。
    pub fn with_tool(mut self, name: impl Into<String>, implementation: ToolImpl) -> Self {
        self.impls.insert(name.into(), implementation);
        self
    }

    /// 整张工具表换掉（测试用：留一个工具就能验「目录里有、表里没有」走 not_found）。
    pub fn with_tools(mut self, tools: HashMap<String, ToolImpl>) -> Self {
        self.impls = tools;
        self
    }

    // ------------------------------------------------------------------ 内部

    fn check_token(&self, ctx: &ToolContext) -> Result<(), ToolFailure> {
        let expected = match &self.token_resolver {
            Some(resolve) => resolve(&ctx.task_id)
                .map_err(|e| ToolFailure::upstream(format!("取 session_token 失败：{e}")))?,
            None => self
                .tokens
                .lock()
                .expect("tokens 锁")
                .get(&ctx.task_id)
                .cloned(),
        };
        let Some(expected) = expected.filter(|t| !t.is_empty()) else {
            return Err(ToolFailure::new(
                ToolErrorCode::Denied,
                format!("task {} 没有登记 session_token", ctx.task_id),
            ));
        };
        if ctx.session_token.is_empty() {
            return Err(ToolFailure::new(
                ToolErrorCode::Denied,
                "调用没带 session_token",
            ));
        }
        // 常数时间比较而不是 ==：token 是随机 32 hex，别给旁路留缝。
        if !constant_time_eq(expected.as_bytes(), ctx.session_token.as_bytes()) {
            return Err(ToolFailure::new(
                ToolErrorCode::Denied,
                format!("session_token 与 task {} 不匹配", ctx.task_id),
            ));
        }
        Ok(())
    }

    /// §3.3：默认 `DEFAULT_TOOL_TIMEOUT_SEC`(60)；`run_python` 用请求里的 timeout_sec + 余量。
    fn budget(&self, name: &str, args: &Map<String, Value>) -> f64 {
        if name != "run_python" {
            return self.default_timeout_sec;
        }
        let timeout_sec = args
            .get("timeout_sec")
            .and_then(Value::as_f64)
            .unwrap_or(120.0);
        timeout_sec + self.run_python_grace_sec
    }

    async fn dispatch(
        &self,
        ctx: &ToolContext,
        req: &ToolCallRequest,
    ) -> Result<ToolOutcome, ToolFailure> {
        self.check_token(ctx)?;

        let Some(spec) = self.specs.iter().find(|t| t.name == req.name) else {
            let mut names: Vec<&str> = self.specs.iter().map(|t| t.name.as_str()).collect();
            names.sort_unstable();
            return Err(ToolFailure::new(
                ToolErrorCode::NotFound,
                format!("没有名为 {:?} 的工具；可用的是 {:?}", req.name, names),
            ));
        };
        let Some(implementation) = self.impls.get(&req.name) else {
            return Err(ToolFailure::new(
                ToolErrorCode::NotFound,
                format!("工具 {:?} 在本次运行里没有实现", req.name),
            ));
        };

        let args = validate_arguments(&spec.parameters, &req.arguments)
            .map_err(|e| ToolFailure::new(ToolErrorCode::InvalidArgs, e.to_string()))?;

        let budget = self.budget(&req.name, &args);
        let env = ToolEnv::new(self.inner.clone(), &ctx.task_id);
        let running = CatchPanic {
            inner: implementation(env, ctx.clone(), args),
        };
        match tokio::time::timeout(Duration::from_secs_f64(budget), running).await {
            Err(_elapsed) => Err(ToolFailure::timeout(format!(
                "工具 {} 超过 {}s 没返回",
                req.name,
                trim_float(budget)
            ))),
            // 这一条是 §3.2「永远不抛异常给调用方」的落点，不是漏网：
            // 工具实现里任何没想到的 panic 都必须变成一个 ToolResult，不能把 worker 打断。
            Ok(Err(panic_message)) => Err(ToolFailure::upstream(format!(
                "工具 {} 内部错误：{panic_message}",
                req.name
            ))),
            Ok(Ok(outcome)) => outcome,
        }
    }

    fn ok_result(req: &ToolCallRequest, outcome: ToolOutcome, started: Instant) -> ToolResult {
        ToolResult {
            call_id: req.call_id.clone(),
            name: req.name.clone(),
            ok: true,
            content: clip(&outcome.content, MAX_TOOL_CONTENT_CHARS),
            data: outcome.data,
            error: None,
            duration_ms: elapsed_ms(started),
            artifacts: outcome.artifacts,
        }
    }

    fn failed_result(req: &ToolCallRequest, failure: ToolFailure, started: Instant) -> ToolResult {
        ToolResult {
            call_id: req.call_id.clone(),
            name: req.name.clone(),
            ok: false,
            // content 也给上：模型只读 content 时不至于一片空白。error.message 不截断。
            content: clip(&failure.message, MAX_TOOL_CONTENT_CHARS),
            data: None,
            error: Some(ToolError {
                code: failure.code,
                message: failure.message,
            }),
            duration_ms: elapsed_ms(started),
            artifacts: Vec::new(),
        }
    }
}

#[async_trait]
impl ToolGateway for P0ToolGateway {
    fn catalog(&self, _ctx: &ToolContext) -> Vec<ToolSpec> {
        // P0 不按 ctx 做任何裁剪（scope / access bundle 是 P1）。返回新 Vec 只是
        // 不想让调用方改到我们手里这份，元素本身就是 gateway_tools() 里那几个。
        self.specs.clone()
    }

    async fn call(&self, ctx: &ToolContext, req: &ToolCallRequest) -> ToolResult {
        let started = Instant::now();
        // 保护圈要罩住**整个** dispatch，不只是工具那个 future：`check_token`（里面
        // 调的是调用方给的 token_resolver 闭包）、`validate_arguments`、算预算的
        // `from_secs_f64` 都在同步段里。RΩ 会把 resolver 接到 SessionStore 上，
        // 那条闭包一 panic 就打穿契约里写死的「永远不失败」（ports.rs）。
        // Python 那边是一个宽 except 罩住从 _check_token 起的全部（tool_gateway.py:133）。
        let outcome = match std::panic::AssertUnwindSafe(self.dispatch(ctx, req))
            .catch_unwind()
            .await
        {
            Ok(r) => r,
            Err(p) => {
                let detail = p
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "（panic 载荷不是字符串）".to_string());
                tracing::error!(tool = %req.name, detail = %detail, "gateway.dispatch_panicked");
                Err(ToolFailure::new(
                    ToolErrorCode::Upstream,
                    format!("工具网关内部出错：{detail}"),
                ))
            }
        };
        match outcome {
            Ok(outcome) => Self::ok_result(req, outcome, started),
            Err(failure) => Self::failed_result(req, failure, started),
        }
    }

    fn register_task(&self, task_id: &str, session_token: &str) {
        if session_token.is_empty() {
            // 失败关闭：空 token 不登记，于是这个 task 的每个调用都 denied。
            tracing::warn!(task_id = %task_id, "gateway.register_task_rejected_empty_token");
            return;
        }
        self.tokens
            .lock()
            .expect("tokens 锁")
            .insert(task_id.to_string(), session_token.to_string());
    }

    fn unregister_task(&self, task_id: &str) {
        self.tokens.lock().expect("tokens 锁").remove(task_id);
    }

    async fn sandbox_id_of(&self, task_id: &str) -> Option<String> {
        self.inner.current_sandbox_id(task_id)
    }

    async fn release_task(&self, task_id: &str) {
        self.unregister_task(task_id);
        self.inner
            .acquire_locks
            .lock()
            .expect("acquire_locks 锁")
            .remove(task_id);
        let sandbox_id = self
            .inner
            .sandbox_ids
            .lock()
            .expect("sandbox_ids 锁")
            .remove(task_id);
        if let (Some(sandbox_id), Some(sandbox)) = (sandbox_id, self.inner.sandbox())
            && let Err(e) = sandbox.release(&sandbox_id).await
        {
            tracing::warn!(task_id = %task_id, sandbox_id = %sandbox_id, error = %e, "gateway.release_failed");
        }
    }
}

/// 逐字节 xor 累加，长度不等也走完再判：别让比较耗时泄露匹配了多少位。
///
/// `black_box` 是编译器屏障（RΩ 补，审核记账 R6）：这一圈的语义等价形式是「首字节不同
/// 就可以提前退出」，优化器完全有权改写成那样，于是常数时间就没了。`black_box` 让
/// 编译器不许对 `diff` 的取值做任何假设，循环也就拆不掉。
///
/// **不为这条加 `subtle` 依赖**（依赖表冻结，而且这一处的威胁模型是本机同进程的
/// 工具调用，不是远程计时攻击）。
fn constant_time_eq(expected: &[u8], got: &[u8]) -> bool {
    let mut diff: u8 = 0;
    for i in 0..expected.len().max(got.len()) {
        let a = expected.get(i).copied().unwrap_or(0);
        let b = got.get(i).copied().unwrap_or(0);
        diff |= a ^ b;
        diff = std::hint::black_box(diff);
    }
    std::hint::black_box(diff) == 0 && expected.len() == got.len()
}

/// 截到恰好 `limit` 个字符（Unicode 标量，不是字节）。
fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let marker = "\n…[内容已截断]";
    let keep = limit.saturating_sub(marker.chars().count());
    let mut out: String = text.chars().take(keep).collect();
    out.push_str(marker);
    out
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

/// 旧实现的 `{budget:g}`：60 写成 60，2.5 还是 2.5。
fn trim_float(value: f64) -> String {
    format!("{value}")
}

/// 包一层 `catch_unwind`：工具 panic 变成一条人话，不是整个进程的墓碑。
struct CatchPanic<T> {
    inner: BoxFuture<'static, T>,
}

impl<T> Future for CatchPanic<T> {
    type Output = Result<T, String>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Self: Unpin（字段是 Pin<Box<..>>），所以 get_mut 是安全的。
        let this = self.get_mut();
        match std::panic::catch_unwind(AssertUnwindSafe(|| this.inner.as_mut().poll(cx))) {
            Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(payload) => Poll::Ready(Err(panic_message(&payload))),
        }
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        format!("panic: {s}")
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        format!("panic: {s}")
    } else {
        "panic: （payload 不是字符串）".to_string()
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    use aite_contracts::SandboxSpec;

    fn gw() -> P0ToolGateway {
        P0ToolGateway::with_optional_ports(None, None, SandboxSpec::new("img"))
    }

    fn args(timeout_sec: Option<f64>) -> Map<String, Value> {
        let mut m = Map::new();
        if let Some(t) = timeout_sec {
            m.insert("timeout_sec".into(), Value::from(t));
        }
        m
    }

    /// `DEFAULT_RUN_PYTHON_GRACE_SEC` 被钉住（RΩ 补，审核记账 R6）。
    ///
    /// 在这之前两条超时测试都把 grace 换成了 0.05 / 0.5，**常量本身改成 0 一条都不会红**。
    /// 而它正是「余量为 0 则外层先赢、模型永远拿不到超时前的部分输出」那条的唯一保障：
    /// 代码的时限由容器内的 coreutils `timeout` 精确执行（超时退出码 124，工具翻成
    /// timeout 并把已经产出的 stdout 带回去）；外层这道只该在 Docker daemon 卡住时开火。
    #[test]
    fn default_run_python_grace_is_five_seconds() {
        // 余量为 0 的话两个时限同时到，赢的通常是外层 —— 模型就永远拿不到超时前的部分输出
        assert_eq!(DEFAULT_RUN_PYTHON_GRACE_SEC, 5.0);
    }

    /// 默认构造出来的 Gateway 真的用的是那个常量，不是别的什么值。
    #[test]
    fn run_python_budget_is_request_timeout_plus_the_default_grace() {
        let g = gw();
        assert_eq!(
            g.budget("run_python", &args(Some(30.0))),
            30.0 + DEFAULT_RUN_PYTHON_GRACE_SEC
        );
        // schema 的默认 timeout_sec 是 120
        assert_eq!(
            g.budget("run_python", &args(None)),
            120.0 + DEFAULT_RUN_PYTHON_GRACE_SEC
        );
    }

    /// 别的工具走 `DEFAULT_TOOL_TIMEOUT_SEC`，不吃 run_python 那份余量。
    #[test]
    fn other_tools_use_the_default_tool_timeout() {
        let g = gw();
        assert_eq!(
            g.budget("list_files", &args(Some(30.0))),
            aite_contracts::DEFAULT_TOOL_TIMEOUT_SEC as f64
        );
    }

    /// `with_run_python_grace` 换得掉（两条超时测试靠它）。
    #[test]
    fn grace_is_injectable() {
        let g = gw().with_run_python_grace(0.25);
        assert_eq!(g.budget("run_python", &args(Some(1.0))), 1.25);
    }
}
