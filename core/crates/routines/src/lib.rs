//! aite-routines —— 例程（CT22、NEW20、NEW21）。owner: EE2（W3）。
//!
//! 现在是空的：CC1（W1）预建这个 crate，把它在工作区清单与 `Cargo.lock` 里的位置
//! 与依赖先占好 —— W3 谁都不改 `Cargo.toml` / `Cargo.lock`（计划 §6.4）。
//!
//! 计划里它会装：
//!
//! - `next_run` 计算：按 Asia/Shanghai 固定 +08:00，不引入时区库
//! - 调度循环（control 内部入口，绕开 `ControlPlane` trait）
//! - `schedule_followup` 等例程工具（实现 gateway 里的 `GatewayTool`，走 catalog）
//! - 输出目标校验：所在群、已授权的公开群、创建者私聊
//!
//! 依赖为什么预声明：`aite-gateway` 是为了实现 `GatewayTool`（原卡定死）；
//! `chrono` 算下次运行时间；其余理由见本 crate 的 `Cargo.toml`。
