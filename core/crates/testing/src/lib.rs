//! aite-testing —— 官方测试替身（对应旧 `aite/testing/**`）。owner: R7
//!
//! `FakePlatform` / `FakeModel` / `FakeSandbox` / `FakeToolGateway` / `FakeSessionStore` /
//! `FakeEvidenceWriter` + `CallLog` + `samples`。
//!
//! **替身只记账、不断言**：谁被以什么参数调用了全进 `CallLog`，断言留给使用方
//! （`aite-evals` 的 expect DSL、各 crate 的测试）。
//!
//! 并行期间别的轨**不要**依赖本 crate（spec §3.2 末：各轨要的替身在自己 crate 的
//! `tests/` 里私写，重复远比冲突便宜）。它是给 `aite-evals` 与 RΩ 的整合测试用的。
pub mod fake_gateway;
pub mod fake_model;
pub mod fake_platform;
pub mod fake_sandbox;
pub mod fake_store;
pub mod jsonschema_mini;
pub mod recorder;
pub mod samples;

pub use fake_gateway::FakeToolGateway;
pub use fake_model::{
    FakeModel, FakeModelError, Repeat, ScriptExhausted, ScriptStep, ScriptedToolCall,
};
pub use fake_platform::{
    FakePlatform, FakePlatformError, HistoryBuilder, fake_p0, history_message,
};
pub use fake_sandbox::{ExecScriptStep, FakeSandbox, FakeSandboxError, as_bytes};
pub use fake_store::{FakeEvidenceWriter, FakeSessionStore};
pub use jsonschema_mini::{SchemaError, validate, validate_arguments};
pub use recorder::{Call, CallLog};
pub use samples::{BUILTINS, CSV_SAMPLE, PNG_1X1, PNG_MAGIC, png_bytes};
