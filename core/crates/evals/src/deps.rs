//! 把场景接到「被测系统」上（对应旧 `aite/evals/wiring.py` 的前半段）。
//!
//! Python 版靠运行时发现 + `getattr` 鸭子探测，Rust 没有这条路，于是换成两件东西：
//!
//! * **注入的 `PlaneFactory`**：谁来造 `ControlPlane` 由调用方给。`aite evals` 二进制在
//!   RΩ 组装之前拿不到真 plane，`None` 就让每个场景以 `phase="wiring"` 收场并说清原因。
//! * **两个 facade trait**：`SandboxFacade` / `GatewayFacade` 把「断言要读的记账面」显式
//!   写进类型里。`--sandbox fake` 是 `FakeSandbox` / `FakeToolGateway`，`--sandbox docker`
//!   是真实现套一层 `real_stack` 的探针，两档下断言读到的是同名同义的东西。
use std::sync::Arc;

use aite_contracts::{AiteConfig, SandboxPort, ToolGateway, ToolResult};
use aite_testing::recorder::CallLog;
use aite_testing::{
    FakeEvidenceWriter, FakeModel, FakePlatform, FakeSandbox, FakeSessionStore, FakeToolGateway,
};
use async_trait::async_trait;
use serde_json::{Map, Value, json};

use crate::protocol_probe::ModelProbe;
use crate::scenario::Scenario;

/// `--sandbox` 认哪几档。默认 `fake` —— 默认路径逐字节不变是硬约束。
pub const SANDBOXES: [&str; 2] = ["fake", "docker"];

/// 带阶段标记的失败。runner 把 phase 一起写进报告，方便看是哪一环断的。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PhaseError {
    pub phase: &'static str,
    pub message: String,
}

impl PhaseError {
    pub fn new(phase: &'static str, message: impl Into<String>) -> Self {
        Self {
            phase,
            message: message.into(),
        }
    }
}

/// 错误 → 一行能读的原因。只取首行，绝不带栈。
pub fn brief(err: &dyn std::fmt::Display) -> String {
    let text = err.to_string();
    text.lines().next().unwrap_or_default().trim().to_string()
}

/// 沙箱那一段的记账面：默认档是 `FakeSandbox`，docker 档是 `SandboxProbe`。
#[async_trait]
pub trait SandboxFacade: SandboxPort {
    fn calls(&self) -> &CallLog;
    /// 还没返回的调用数。真沙箱 acquire/exec 要秒级，`settle()` 得减掉这一段。
    fn in_flight(&self) -> usize {
        0
    }
    /// 收摊：docker 档要靠它把容器和客户端收干净；默认档什么都不做。
    async fn close(&self) {}
}

#[async_trait]
impl SandboxFacade for FakeSandbox {
    fn calls(&self) -> &CallLog {
        &self.calls
    }
}

/// Gateway 那一段的记账面：`checks::gateway_calls` / `gateway_result` 两档共用。
pub trait GatewayFacade: ToolGateway {
    fn calls(&self) -> &CallLog;
    fn count(&self, name: &str) -> usize;
    fn results_of(&self, name: &str) -> Vec<ToolResult>;
}

impl GatewayFacade for FakeToolGateway {
    fn calls(&self) -> &CallLog {
        &self.calls
    }
    fn count(&self, name: &str) -> usize {
        FakeToolGateway::count(self, name)
    }
    fn results_of(&self, name: &str) -> Vec<ToolResult> {
        FakeToolGateway::results_of(self, name)
    }
}

/// 一个场景要用到的全套替身。
pub struct Deps {
    pub config: AiteConfig,
    pub platform: Arc<FakePlatform>,
    /// 永远是探针：它记账每一步出牌，也补上了真模型客户端没有的 `calls` / `call_count`
    pub model: Arc<ModelProbe>,
    pub sandbox: Arc<dyn SandboxFacade>,
    pub gateway: Arc<dyn GatewayFacade>,
    pub store: Arc<FakeSessionStore>,
    pub evidence: Arc<FakeEvidenceWriter>,
}

impl Deps {
    /// 收摊。真沙箱那一档要靠它把容器和客户端收干净。
    ///
    /// 任务正常收尾时容器已经由 worker 的 `release_task` 还掉了，这里管的是没走到终态
    /// 的那些 —— 场景超时、步数上限打断、断言前就炸了。不收的话容器要挂到 `reap_idle`
    /// 的 `idle_sec`（默认 300s）才被捡走。
    pub async fn close(&self) {
        self.sandbox.close().await;
    }

    /// 所有替身被调用的总次数 —— 用来判断系统是不是已经不干活了。
    pub fn activity(&self) -> usize {
        self.platform.calls.len()
            + self.model.calls.len()
            + self.sandbox.calls().len()
            + self.gateway.calls().len()
            + self.store.calls.len()
            + self.evidence.calls.len()
    }

    pub fn stats(&self) -> Map<String, Value> {
        aite_testing::kwargs! {
            "platform_calls" => json!(self.platform.calls.len()),
            "model_calls" => json!(self.model.call_count()),
            "gateway_calls" => json!(self.gateway.calls().count("call")),
            "sandbox_calls" => json!(self.sandbox.calls().len()),
            "sessions" => json!(self.store.session_count()),
            "tasks" => json!(self.store.task_count()),
        }
    }
}

/// Gateway 的令牌解析器（与 `aite_gateway::TokenResolver` 同形；evals 不依赖那个 crate）。
pub type TokenResolver = Arc<dyn Fn(&str) -> Result<Option<String>, String> + Send + Sync>;

/// 从替身 store 直接读 `session_token`，给 `P0ToolGateway` 用。
///
/// 两件事非这么做不可：
///
/// 1. **不走 `store.get_task()`**：那是 async，而 `TokenResolver` 是同步签名；更要命的是
///    它会往 store 的记账里加一笔，而 `settle()` 的静默判据数的就是这些记账 ——
///    每次工具调用都碰它，评测就永远等不到静默。所以走不记账的 `tasks_snapshot()`。
/// 2. **一定要接上**：`P0ToolGateway` 的令牌校验是失败关闭的（契约 ports.rs 写死），
///    不接就是每个工具调用都 `denied`，而报出来的是「denied」不是「你没接 token_resolver」。
///    正因为这个失败形态太难查，`SandboxFactory` 把它做成了必传参数。
pub fn token_resolver_of(store: &Arc<FakeSessionStore>) -> TokenResolver {
    let store = Arc::clone(store);
    Arc::new(move |task_id: &str| {
        Ok(store
            .tasks_snapshot()
            .into_iter()
            .find(|t| t.id == task_id)
            .map(|t| t.session_token))
    })
}

/// 造 sandbox + gateway 那一段的工厂（`--sandbox docker` 的接线点，RΩ 注入真实现）。
///
/// 第四个参数是现成的 `TokenResolver`：RΩ 把它塞进 `P0ToolGateway` 即可。
/// 做成必传是刻意的 —— 忘了接的后果是每个工具调用都 denied，查起来完全不着边际。
pub type SandboxFactory = Arc<
    dyn Fn(
            &Arc<FakePlatform>,
            &Arc<FakeSessionStore>,
            &AiteConfig,
            &TokenResolver,
        ) -> Result<(Arc<dyn SandboxFacade>, Arc<dyn GatewayFacade>), String>
        + Send
        + Sync,
>;

/// 造模型的工厂（`--model live` 的接线点，RΩ 注入 `aite-models`）。
/// 每个场景调一次 —— 观测要按场景切开。
pub type ModelFactory =
    Arc<dyn Fn() -> Result<Arc<dyn aite_contracts::ModelPort>, String> + Send + Sync>;

/// `build_deps` 的可选注入项。默认全 None = 纯替身那一档。
#[derive(Clone, Default)]
pub struct DepsOptions {
    /// `fake`（默认）/ `docker`
    pub sandbox_kind: String,
    /// docker 档怎么造真沙箱与真 Gateway；`--sandbox docker` 且没给 = 一行人话 + 退出 2
    pub sandbox_factory: Option<SandboxFactory>,
    /// 不给就按场景的 `model_script` 造 `FakeModel`
    pub model: Option<Arc<dyn aite_contracts::ModelPort>>,
}

impl DepsOptions {
    pub fn fake() -> Self {
        Self {
            sandbox_kind: "fake".to_string(),
            ..Self::default()
        }
    }
}

/// 按场景里的 fixture 造一套替身。
///
/// `sandbox_kind` 选沙箱与 Gateway 这一段接谁（`--sandbox`）：
///
/// * `fake`（默认）—— `FakeSandbox` 照场景的 `exec_script` 演，`FakeToolGateway` 自己实现
///   工具。**这一档逐字节不变**：`check.sh` 的 B8、CI、本轨的测试都吃这条路。
/// * `docker` —— 真沙箱 + 真 Gateway，代码在容器里真跑。接线点由 `sandbox_factory` 注入。
///
/// `model` 不给就按场景的 `model_script` 造 `FakeModel`；`--model live` 会从这里塞进来一个
/// 真客户端。**两种都统一套一层 `ModelProbe`**：`activity()` 读 `model.calls`、`stats()` 读
/// `model.call_count`，真模型客户端两样都没有（契约 `ModelPort` 里也确实没有，它们是替身
/// 的记账面）。顺带把每一步出牌记下来，`analyze()` 据此出实测报告。
pub fn build_deps(sc: &Scenario, options: &DepsOptions) -> Result<Deps, PhaseError> {
    let kind = if options.sandbox_kind.is_empty() {
        "fake"
    } else {
        options.sandbox_kind.as_str()
    };
    if !SANDBOXES.contains(&kind) {
        return Err(PhaseError::new(
            "wiring",
            format!("不认识的 --sandbox {kind:?}，只认 {SANDBOXES:?}"),
        ));
    }

    let config = config_of(sc)?;
    let files = sc
        .build_files()
        .map_err(|e| PhaseError::new("wiring", format!("platform.files 读不出来：{}", e.0)))?;
    let platform = Arc::new(
        FakePlatform::new()
            .with_history(sc.build_history())
            .with_documents(sc.build_documents())
            .with_files(files),
    );
    let store = Arc::new(FakeSessionStore::new());

    let (sandbox, gateway): (Arc<dyn SandboxFacade>, Arc<dyn GatewayFacade>) = if kind == "docker" {
        let factory = options.sandbox_factory.clone().ok_or_else(|| {
            PhaseError::new(
                "wiring",
                "--sandbox docker 还没接线（真沙箱与真 Gateway 由 RΩ 通过 SandboxFactory 注入）",
            )
        })?;
        let resolver = token_resolver_of(&store);
        factory(&platform, &store, &config, &resolver).map_err(|e| PhaseError::new("wiring", e))?
    } else {
        let sandbox = Arc::new(FakeSandbox::new(sc.sandbox.exec_script.clone()));
        let gateway = Arc::new(
            FakeToolGateway::new(platform.clone(), sandbox.clone())
                .with_sandbox_image(&config.sandbox.image),
        );
        (sandbox, gateway)
    };

    let inner: Arc<dyn aite_contracts::ModelPort> = match &options.model {
        Some(m) => m.clone(),
        None => Arc::new(FakeModel::new(sc.model_script.clone())),
    };
    Ok(Deps {
        config,
        platform,
        model: Arc::new(ModelProbe::new(inner)),
        sandbox,
        gateway,
        store,
        evidence: Arc::new(FakeEvidenceWriter::new()),
    })
}

/// 场景的 `config:` 片段叠在 `platform: fake` 之上（与 Python 的
/// `AiteConfig.model_validate({"platform": "fake", **config})` 同义）。
pub fn config_of(sc: &Scenario) -> Result<AiteConfig, PhaseError> {
    let mut map = Map::new();
    map.insert("platform".to_string(), json!("fake"));
    for (k, v) in &sc.config {
        map.insert(k.clone(), v.clone());
    }
    serde_json::from_value(Value::Object(map))
        .map_err(|e| PhaseError::new("wiring", format!("场景的 config 片段读不出来：{e}")))
}

/// 模型这条路上还有活没干完？—— `settle()` 的静默判据要减掉这一段。
///
/// `Deps::activity()` 是个「变没变」的探测器，看不见长时间的 await。脚本化替身瞬时返回，
/// 这从来没露过馅；**真模型一次 chat 动辄几秒**，期间一个替身都不会被碰 —— 照 150ms 的
/// 静默判据，系统在第一次回包之前就被判定「不干活了」，任务当场被取消。
///
/// 两种情况算忙：
/// 1. 有 chat 还没返回（`ModelProbe::in_flight`）；
/// 2. 上一发报错了、且还有任务没落终态 —— worker 正睡在退避里等着重试。重试用尽时
///    worker 把任务判 failed，任务一落终态这里就不再算忙，所以**不用在评测侧写死退避
///    时长**（那个数是 worker 的，抄一份迟早对不上）。
pub fn model_busy(deps: &Deps) -> bool {
    if deps.model.in_flight() > 0 {
        return true;
    }
    if !deps.model.awaiting_retry() {
        return false;
    }
    deps.store
        .tasks_snapshot()
        .iter()
        .any(|t| t.status.is_active())
}

/// 沙箱这条路上还有活没干完？—— 与 `model_busy` 同一个理由，换成沙箱那一头。
pub fn sandbox_busy(deps: &Deps) -> bool {
    deps.sandbox.in_flight() > 0
}

/// 系统还在等外部返回吗（模型 / 沙箱）。
pub fn busy(deps: &Deps) -> bool {
    model_busy(deps) || sandbox_busy(deps)
}
