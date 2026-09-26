//! 功能接缝（CC4 ⑤）：每个功能**只改自己那一个** `features/<x>.rs`，谁都不改这个文件。
//!
//! 两段：
//!
//! ```text
//! 组装前  wire(&mut FeatureCtx) -> Result<(), String>   登记工具、推 gateway / worker 选项、写模型覆盖槽
//! 组装后  start(&StartCtx)                               起循环 / 监听（同步；要起就 tokio::spawn，不许阻塞）
//! ```
//!
//! [`wire_all`] / [`start_all`] 按**固定顺序**调每个文件（照卡片顺序）：
//!
//!  1. `stores`（DD1）
//!  2. `models`（DD7）
//!  3. `admin`（DD12）
//!  4. `memory`（EE1）
//!  5. `routines`（EE2）
//!  6. `search`（EE4）
//!  7. `git`（EE5）
//!  8. `pages`（EE6（W6 的 HH2 也写它））
//!  9. `budget`（EE3）
//! 10. `audit`（EE12）
//! 11. `retention`（EE12）
//! 12. `purge`（FF6）
//! 13. `connections`（FF8）
//! 14. `approvals`（EE7）
//! 15. `compliance`（EE12）
//! 16. `sandbox`（DD6）
//! 17. `egress`（EE10）
//! 18. `personal`（HH1）
//!
//! **登记出错的唯一通道**（后面各轨照抄这个签名）：`ToolRegistry::register` 当场回 `Err` →
//! 功能文件的 `wire` 用 `?` 往上抛 → [`wire_all`] 原样传出（遇到第一个 `Err` 就停）→
//! `build_app_with_features` 统一映射成 `StartupError`、评测那一路的 `plane_factory` 映射成它自己的
//! `Err(String)`。不在 `wire` 里 `expect()`、不吞。
//!
//! **wire 阶段不许做任何 I/O**：`build_app` 只组装（`build_app_contract.rs` 钉着），store 此刻还没
//! `init()`（建表在 `run.rs` 起飞时）。
//!
//! [`start_all`] 的调用点本该在 `run.rs` 的 `takeoff` 里、`platform.start` 之后 —— `run.rs` 不归 CC4，
//! 调用点记账转 T0c（EE7 / EE2 / DD12 依赖它）。今天没有调用点，全是空壳，不调它行为不变。
use std::sync::Arc;

use aite_contracts::{AiteConfig, ControlPlane, ModelPort, PlatformPort, SessionStore};
use aite_gateway::{GatewayBuilder, ToolRegistry};
use aite_worker::WorkerDeps;

mod admin;
mod approvals;
mod audit;
mod budget;
mod compliance;
mod connections;
mod egress;
mod git;
mod memory;
mod models;
mod pages;
mod personal;
mod purge;
mod retention;
mod routines;
mod sandbox;
mod search;
mod stores;

/// 一条网关变更（按推入顺序在建网关时依次应用）。
pub type GatewayOption = Box<dyn FnOnce(GatewayBuilder) -> GatewayBuilder + Send>;
/// 一条 worker 变更（在 `AgentWorker::new` 之前依次应用到 `WorkerDeps`）。
pub type WorkerOption = Box<dyn FnOnce(&mut WorkerDeps) + Send>;

/// 组装前各功能能碰到的东西。`services` 字段由 T0c 加（`Services` 类型要 T0 才有）。
pub struct FeatureCtx {
    /// 配置（只读用；改配置不是功能的事）
    pub config: AiteConfig,
    /// `build_app` 第 6 步开的那个 store（此刻还没 `init()`，别在 wire 里读写）
    pub store: Arc<dyn SessionStore>,
    /// 外部工具登记（进网关目录，排在内建五个之后）
    pub registry: ToolRegistry,
    /// 模型覆盖槽：`Some` 就替掉按 config 造的那个；**注入的模型仍优先**（给了就用给的）
    pub model_override: Option<Arc<dyn ModelPort>>,
    /// 有序的网关变更
    pub gateway_options: Vec<GatewayOption>,
    /// 有序的 worker 变更
    pub worker_options: Vec<WorkerOption>,
}

impl FeatureCtx {
    pub fn new(config: AiteConfig, store: Arc<dyn SessionStore>) -> Self {
        Self {
            config,
            store,
            registry: ToolRegistry::new(),
            model_override: None,
            gateway_options: Vec::new(),
            worker_options: Vec::new(),
        }
    }

    /// 登记完、选项都推齐之后建网关：`base.with_registry(登记)`，再依次应用 `gateway_options`。
    pub fn build_gateway(
        registry: ToolRegistry,
        options: Vec<GatewayOption>,
        base: GatewayBuilder,
    ) -> GatewayBuilder {
        options
            .into_iter()
            .fold(base.with_registry(registry), |gw, option| option(gw))
    }
}

/// 组装后各功能能碰到的东西。
pub struct StartCtx {
    pub plane: Arc<dyn ControlPlane>,
    pub platform: Arc<dyn PlatformPort>,
}

/// 按固定顺序调每个功能的 `wire`，遇到第一个 `Err` 就原样返回。
pub fn wire_all(ctx: &mut FeatureCtx) -> Result<(), String> {
    stores::wire(ctx)?;
    models::wire(ctx)?;
    admin::wire(ctx)?;
    memory::wire(ctx)?;
    routines::wire(ctx)?;
    search::wire(ctx)?;
    git::wire(ctx)?;
    pages::wire(ctx)?;
    budget::wire(ctx)?;
    audit::wire(ctx)?;
    retention::wire(ctx)?;
    purge::wire(ctx)?;
    connections::wire(ctx)?;
    approvals::wire(ctx)?;
    compliance::wire(ctx)?;
    sandbox::wire(ctx)?;
    egress::wire(ctx)?;
    personal::wire(ctx)?;
    Ok(())
}

/// 按固定顺序调每个功能的 `start`。
pub fn start_all(ctx: &StartCtx) {
    stores::start(ctx);
    models::start(ctx);
    admin::start(ctx);
    memory::start(ctx);
    routines::start(ctx);
    search::start(ctx);
    git::start(ctx);
    pages::start(ctx);
    budget::start(ctx);
    audit::start(ctx);
    retention::start(ctx);
    purge::start(ctx);
    connections::start(ctx);
    approvals::start(ctx);
    compliance::start(ctx);
    sandbox::start(ctx);
    egress::start(ctx);
    personal::start(ctx);
}

/// 两处组装（`build_app_with_features` 与评测的 `plane_factory`）共用的那一步：
/// 建 `FeatureCtx` 并跑完 `wire_all`。
pub fn wire_features(
    config: AiteConfig,
    store: Arc<dyn SessionStore>,
) -> Result<FeatureCtx, String> {
    let mut ctx = FeatureCtx::new(config, store);
    wire_all(&mut ctx)?;
    Ok(ctx)
}

/// 两处组装共用：在 `AgentWorker::new` 之前把 worker 选项依次应用到 `WorkerDeps`。
pub fn apply_worker_options(deps: &mut WorkerDeps, options: Vec<WorkerOption>) {
    for option in options {
        option(deps);
    }
}
