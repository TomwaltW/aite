//! aite-memory —— 记忆（CT21、NEW22）。owner: EE1（W3）。
//!
//! 现在是空的：CC1（W1）预建这个 crate，把它在工作区清单与 `Cargo.lock` 里的位置
//! 与依赖先占好 —— W3 谁都不改 `Cargo.toml` / `Cargo.lock`（计划 §6.4）。
//!
//! 计划里它会装：
//!
//! - 按群 / 工作区的记忆存储（每群一份，SQLite）；公开群进工作区、私有群自存，
//!   钉钉 / 企微一律按私有群
//! - `memory_read` / `memory_write` 工具（实现 gateway 里的 `GatewayTool`，走 catalog）
//! - 写入过滤：只存工作事实、带来源 task_id，拒收身份证号 / 手机号一类
//!
//! 依赖为什么预声明：`aite-gateway` 是为了实现 `GatewayTool`（原卡定死）；
//! `rusqlite` 是记忆文件的存储；其余理由见本 crate 的 `Cargo.toml`。
