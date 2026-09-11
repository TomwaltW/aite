//! aite-evals —— 评测 runner / 场景 / checks / protocol probe / demo-fixture
//! （对应旧 `aite/evals/**` 与 `scripts/demo_fixture.py`）。owner: R7
//!
//! 分七层：
//!
//! ```text
//! scenario.rs        evals/p0/*.yaml 的形状与加载（原文件不动，这里必须原样读得懂）
//! deps.rs            把替身接到被测系统上；ControlPlane 由注入的 PlaneFactory 给
//! checks.rs          expect 断言 DSL
//! protocol_probe.rs  看模型按不按协议出牌（--protocol-report）
//! real_stack.rs      --sandbox docker / --model live 那两档的接线点与探针
//! runner.rs          跑一个场景 / 一整套，产出 JSON 摘要
//! demo_plane.rs      只为自证的最小 ControlPlane（01/02/09/10 跑绿）
//! ```
//!
//! `cli::run(args)` 是 `aite evals` 的子命令入口；RΩ 组装时改用
//! `cli::run_with_wiring(args, &Wiring{plane, model, sandbox})` 把真实现注进来。
pub mod checks;
pub mod cli;
pub mod demo_fixture;
pub mod demo_plane;
pub mod deps;
pub mod protocol_probe;
pub mod real_stack;
pub mod regex_mini;
pub mod runner;
pub mod scenario;

pub use checks::{CheckError, run_check, run_checks};
pub use demo_plane::DemoPlane;
pub use deps::{
    Deps, DepsOptions, GatewayFacade, ModelFactory, PhaseError, SandboxFacade, SandboxFactory,
    build_deps,
};
pub use protocol_probe::{ModelProbe, Observation, ToolCallObservation, analyze, render_digest};
pub use real_stack::{GatewayProbe, SandboxProbe};
pub use runner::{
    PlaneFactory, RunOptions, ScenarioResult, SuiteResult, run_scenario, run_suite, settle,
};
pub use scenario::{EventAfter, EventSpec, Scenario, ScenarioError, load_scenario, load_suite};
