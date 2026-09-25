//! aite-githost —— Git 托管（CT12、CT15、NEW24）。owner: EE5（W3）。
//!
//! 现在是空的：CC1（W1）预建这个 crate，把它在工作区清单与 `Cargo.lock` 里的位置
//! 与依赖先占好 —— W3 谁都不改 `Cargo.toml` / `Cargo.lock`（计划 §6.4）。
//!
//! 计划里它会装：
//!
//! - `GitHost` 端口与 GitLab / 极狐适配：服务端拉归档进 `/work/repo`、提交 API 建分支、
//!   开 `Draft: ` MR 并反链话题；MR 作者是 bot 用户，项目访问令牌不进沙箱
//! - Gitee 只做桩；GitHub / Codeup / Gitea 在 P1 报「未实现」
//! - git 相关工具（实现 gateway 里的 `GatewayTool`，走 catalog）
//!
//! 依赖为什么预声明：`aite-gateway` 是为了实现 `GatewayTool`（原卡定死）；
//! `reqwest` 调 GitLab API；其余理由见本 crate 的 `Cargo.toml`。
//! 注意：归档若是 tar.gz，解包要的 crate 不在 `[workspace.dependencies]` 里，
//! 那是新第三方依赖，EE5 届时要停下报告（CC1 回执已记账）。
