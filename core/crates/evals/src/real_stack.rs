//! `--sandbox docker` / `--model live` 那两档的接线点（对应旧 `aite/evals/real_stack.py`）。
//!
//! 评测默认跑的是全替身：`FakeSandbox` 按 `exec_script` 演、`FakeToolGateway` 自己实现工具。
//! docker 那一档换成**真**沙箱 + **真** `P0ToolGateway`，好让「附件 → download_attachment
//! → run_python 画图 → final(artifacts) → send_file」这条主干在评测里真的走一遍。
//! 平台仍然是 `FakePlatform` —— 附件从它来，回执也发回它，验的是沙箱与 Gateway 这一段。
//!
//! 两件事得在这里补上，都不是别人漏了，是「只有接线方才知道」的：
//!
//! **一、断言面。** 场景的 `expect` 读 `deps.sandbox.calls()` / `deps.gateway.count()` /
//! `results_of()` —— 那些是替身的记账面，真实现没有（契约 `SandboxPort` / `ToolGateway`
//! 里也确实没有，它们不是契约的一部分）。所以这里给两个透明包装 `SandboxProbe` /
//! `GatewayProbe`，跟 `ModelProbe` 之于 live 模型是同一件事：照原样转发，顺手记账。
//!
//! **二、`session_token`。** 真 Gateway 的令牌校验是失败关闭的：`register_task()` 现场登记，
//! 或构造时给 `token_resolver`，两个都没有 = 每个工具调用都 `denied`。评测这边走后者：
//! `token_resolver_of` 直接读 `FakeSessionStore` 的字段（同步、且**不走 store 的方法**
//! —— `get_task()` 会往 `store.calls` 记一笔，而 `Deps::activity()` 数的就是这些记账，
//! 每次工具调用都去碰它，`settle()` 的静默判据就永远不静默）。
//!
//! **并行期间 R2（真沙箱）与 R6（真 Gateway）都还不存在**，所以 `build_docker_stack` 是
//! 一个注入点：RΩ 把 `SandboxFactory` 传进来，这里的探针原样可用。没注入 = 一行人话
//! + 退出码 2，绝不让十个场景各自烂在第一个工具调用上。
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use aite_contracts::{
    ExecRequest, ExecResult, SandboxError, SandboxPort, SandboxSpec, ToolCallRequest, ToolContext,
    ToolGateway, ToolResult, ToolSpec,
};
use aite_testing::recorder::CallLog;
use aite_testing::{FakeSessionStore, kwargs};
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::deps::{GatewayFacade, SandboxFacade};
use crate::scenario::Scenario;

/// `SandboxPort` 的透明包装：照原样转发，顺手把每次调用记进 `calls`。
///
/// 方法名与参数名跟 `FakeSandbox` 记的那套逐字对齐 —— `check: sandbox_calls` 的
/// `method: release` 这类断言两档下读到的是同一个键。
///
/// **`in_flight` 是这一档能跑起来的前提**，跟 `ModelProbe::in_flight` 之于 live 模型一模一样：
/// `settle()` 的静默判据是「所有替身的记账都不再变」，它看不见长时间的 await。替身沙箱
/// 瞬时返回，这从来没露过馅；真沙箱 `acquire` 一次要起容器 + 探路一秒多，这期间一个替身
/// 都不会被碰 —— 照 150ms 的判据，任务在第一个容器建好之前就被判定「不干活了」，当场被取消。
///
/// 调用记在**执行前**（`ModelProbe` 同理）：报错的那次也得留下痕迹，不然错在哪数都数不出来。
pub struct SandboxProbe {
    pub inner: Arc<dyn SandboxPort>,
    pub calls: CallLog,
    in_flight: AtomicUsize,
}

impl SandboxProbe {
    pub fn new(inner: Arc<dyn SandboxPort>) -> Self {
        Self {
            inner,
            calls: CallLog::new(),
            in_flight: AtomicUsize::new(0),
        }
    }

    async fn track<T, E: std::fmt::Display>(
        &self,
        idx: usize,
        fut: impl std::future::Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        let result = fut.await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        if let Err(e) = &result {
            self.calls.set_error(idx, e.to_string());
        }
        result
    }
}

#[async_trait]
impl SandboxPort for SandboxProbe {
    async fn acquire(&self, task_id: &str, spec: &SandboxSpec) -> Result<String, SandboxError> {
        let idx = self.calls.record(
            "acquire",
            kwargs! {"task_id" => json!(task_id), "image" => json!(spec.image)},
        );
        let sandbox_id = self.track(idx, self.inner.acquire(task_id, spec)).await?;
        self.calls.set_result(idx, json!(sandbox_id));
        Ok(sandbox_id)
    }

    async fn exec(&self, sandbox_id: &str, req: &ExecRequest) -> Result<ExecResult, SandboxError> {
        let idx = self.calls.record(
            "exec",
            kwargs! {"sandbox_id" => json!(sandbox_id), "timeout_sec" => json!(req.timeout_sec)},
        );
        let result = self.track(idx, self.inner.exec(sandbox_id, req)).await?;
        self.calls
            .set_result(idx, json!(format!("exit_code={}", result.exit_code)));
        Ok(result)
    }

    async fn put_file(
        &self,
        sandbox_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), SandboxError> {
        let idx = self.calls.record(
            "put_file",
            kwargs! {
                "sandbox_id" => json!(sandbox_id),
                "path" => json!(path),
                "size" => json!(data.len()),
            },
        );
        self.track(idx, self.inner.put_file(sandbox_id, path, data))
            .await
    }

    async fn get_file(&self, sandbox_id: &str, path: &str) -> Result<Vec<u8>, SandboxError> {
        let idx = self.calls.record(
            "get_file",
            kwargs! {"sandbox_id" => json!(sandbox_id), "path" => json!(path)},
        );
        let data = self
            .track(idx, self.inner.get_file(sandbox_id, path))
            .await?;
        self.calls
            .set_result(idx, json!(format!("{} bytes", data.len())));
        Ok(data)
    }

    async fn list_files(&self, sandbox_id: &str) -> Result<Vec<String>, SandboxError> {
        let idx = self
            .calls
            .record("list_files", kwargs! {"sandbox_id" => json!(sandbox_id)});
        self.track(idx, self.inner.list_files(sandbox_id)).await
    }

    async fn touch(&self, sandbox_id: &str) -> Result<(), SandboxError> {
        let idx = self
            .calls
            .record("touch", kwargs! {"sandbox_id" => json!(sandbox_id)});
        self.track(idx, self.inner.touch(sandbox_id)).await
    }

    async fn release(&self, sandbox_id: &str) -> Result<(), SandboxError> {
        let idx = self
            .calls
            .record("release", kwargs! {"sandbox_id" => json!(sandbox_id)});
        self.track(idx, self.inner.release(sandbox_id)).await
    }

    async fn reap_idle(&self, idle_sec: u32) -> Result<Vec<String>, SandboxError> {
        let idx = self
            .calls
            .record("reap_idle", kwargs! {"idle_sec" => json!(idle_sec)});
        self.track(idx, self.inner.reap_idle(idle_sec)).await
    }

    async fn close_all(&self) -> Result<(), SandboxError> {
        let idx = self.calls.record("close_all", kwargs! {});
        self.track(idx, self.inner.close_all()).await
    }
}

#[async_trait]
impl SandboxFacade for SandboxProbe {
    fn calls(&self) -> &CallLog {
        &self.calls
    }

    fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// 把本进程还开着的容器收干净。runner 在场景收尾时调，见 `Deps::close`。
    async fn close(&self) {
        let _ = SandboxPort::close_all(self).await;
    }
}

/// `ToolGateway` 的透明包装：转发 `catalog` / `call`，补上断言要的记账面。
///
/// `count()` / `results_of()` / `calls()` 与 `FakeToolGateway` 同名同义 —— `checks.rs` 的
/// `gateway_calls` / `gateway_result` 两个 check 两档下读到的是同一套东西。
/// `register_task` / `sandbox_id_of` / `release_task` 原样转发给真实现。
pub struct GatewayProbe {
    pub inner: Arc<dyn ToolGateway>,
    pub calls: CallLog,
    results: Mutex<Vec<ToolResult>>,
}

impl GatewayProbe {
    pub fn new(inner: Arc<dyn ToolGateway>) -> Self {
        Self {
            inner,
            calls: CallLog::new(),
            results: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ToolGateway for GatewayProbe {
    fn catalog(&self, ctx: &ToolContext) -> Vec<ToolSpec> {
        self.calls
            .record("catalog", kwargs! {"task_id" => json!(ctx.task_id)});
        self.inner.catalog(ctx)
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
        let result = self.inner.call(ctx, req).await;
        self.calls.set_result(
            idx,
            json!(match &result.error {
                None => format!("ok={}", result.ok),
                Some(e) => format!("ok={} {}", result.ok, e.code.as_str()),
            }),
        );
        self.results
            .lock()
            .expect("GatewayProbe 锁")
            .push(result.clone());
        result
    }

    fn register_task(&self, task_id: &str, session_token: &str) {
        self.inner.register_task(task_id, session_token);
    }

    fn unregister_task(&self, task_id: &str) {
        self.inner.unregister_task(task_id);
    }

    async fn sandbox_id_of(&self, task_id: &str) -> Option<String> {
        self.inner.sandbox_id_of(task_id).await
    }

    async fn release_task(&self, task_id: &str) {
        self.inner.release_task(task_id).await;
    }
}

impl GatewayFacade for GatewayProbe {
    fn calls(&self) -> &CallLog {
        &self.calls
    }

    fn count(&self, name: &str) -> usize {
        self.calls
            .of("call")
            .iter()
            .filter(|c| c.arg_str("name") == Some(name))
            .count()
    }

    fn results_of(&self, name: &str) -> Vec<ToolResult> {
        self.results
            .lock()
            .expect("GatewayProbe 锁")
            .iter()
            .filter(|r| r.name == name)
            .cloned()
            .collect()
    }
}

// --------------------------------------------------------------------------

/// task_id -> `Task.session_token` 的同步查询（真 Gateway 的 token_resolver 形状）。
pub type TokenResolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// task_id -> `Task.session_token`，直接读 store 的字段。
///
/// 两点讲究：**同步**（真 Gateway 的 token resolver 是同步签名，而
/// `SessionStore::get_task` 是 async）；**不走 store 的方法**（记账会让 settle 永不静默）。
pub fn token_resolver_of(store: Arc<FakeSessionStore>) -> TokenResolver {
    Arc::new(move |task_id: &str| store.session_token_of(task_id))
}

/// 跟真机 `_sandbox_spec` 同一口径 —— 真机怎么从 config 出 spec，这里就怎么出。
pub fn sandbox_spec_of(config: &aite_contracts::AiteConfig) -> SandboxSpec {
    SandboxSpec {
        image: config.sandbox.image.clone(),
        cpu: config.sandbox.cpu,
        mem_mb: config.sandbox.mem_mb,
        network: aite_contracts::SandboxNetwork::None,
        workdir: "/work".to_string(),
    }
}

/// 哪些场景写了 `sandbox.exec_script`。
///
/// 那是 `FakeSandbox` 的台词，docker 档下**一律忽略**：代码交给真容器跑，本来就没有
/// 「照脚本回一个 exit_code」这回事。选忽略而不是报错，是因为唯一带 exec_script 的靶心
/// 场景就是 04_csv_to_chart —— 报错的话这一档连它都跑不了。忽略但要留痕：CLI 拿这个
/// 名单在起飞时打一行 stderr 点名。
pub fn scenarios_with_exec_script(scenarios: &[Scenario]) -> Vec<String> {
    scenarios
        .iter()
        .filter(|sc| !sc.sandbox.exec_script.is_empty())
        .map(|sc| sc.name.clone())
        .collect()
}

/// 真体检的实现：连 daemon、查镜像。没问题回 `None`，有问题回一行人话。
/// 由 RΩ 注入 —— core 这侧不认识 Docker，真沙箱在 edge（Go）那边。
pub type DockerProbe = std::sync::Arc<dyn Fn(&[String]) -> Option<String> + Send + Sync>;

/// 起飞前体检：daemon 连得上吗、镜像在吗。没问题回 `None`，有问题回一行给人看的话。
///
/// `probe` 为 None = 还没接线，回一句「还没接线」。
///
/// **不能**写成「接线了就 return None」：那样 RΩ 一注入 SandboxFactory，体检就整个
/// 消失了 —— 回到「daemon 没起 / 镜像没 build 时十个场景各自烂在第一个工具调用上」，
/// 而这个函数存在的全部理由，就是在起飞前把这句话一次说清楚。
pub fn docker_preflight(images: &[String], probe: Option<&DockerProbe>) -> Option<String> {
    if let Some(probe) = probe {
        return probe(images);
    }
    let mut names: Vec<&String> = images.iter().collect();
    names.sort();
    names.dedup();
    Some(format!(
        "--sandbox docker 还没接线：真沙箱在 edge（Go，R2）、真 Gateway 在 R6，\
         由 RΩ 通过 SandboxFactory 注入后这一档才跑得动（要用的镜像：{names:?}）"
    ))
}
