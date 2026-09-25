//! aite-admin —— 管理台 v0（CT24 的配对与上线页、CT16 的 Scope / Bundle 编辑、
//! CT27 的审计页）。owner: DD12（W2），之后 EE9 / FF7 往上加页面。
//!
//! 现在是空的：CC1（W1）预建这个 crate，只为把它在工作区清单与 `Cargo.lock` 里的
//! 位置先占好，免得 W2 / W3 好几轨同时改 `Cargo.toml` / `Cargo.lock`（计划 §6.4）。
//!
//! 计划里它会装：
//!
//! - `aite admin serve` 的 axum 服务端渲染（D4：只用 Rust，无前端构建链）
//! - 平台 OAuth 与一次性引导令牌登录，等保二级的登录锁定、会话超时、CSRF、转义
//! - Scope / Bundle / Skills 编辑页、任务证据只读页、「模型与备案」面板
//!
//! 依赖为什么这么少：DD12 本来就要加 axum 并改锁（计划 D4 / D12 已批），
//! 其余缺的届时一并加；预声明的理由见本 crate 的 `Cargo.toml`。
