//! aite-search —— 联网搜索与工作区搜索（CT08、CT11、CT19）。owner: EE4（W3）。
//!
//! 现在是空的：CC1（W1）预建这个 crate，把它在工作区清单与 `Cargo.lock` 里的位置
//! 与依赖先占好 —— W3 谁都不改 `Cargo.toml` / `Cargo.lock`（计划 §6.4）。
//!
//! 计划里它会装：
//!
//! - `SearchPort`：默认博查或智谱 web search，百度千帆兜底；在服务端跑、不经沙箱代理，
//!   密钥走环境变量，结果包成 `<external>` 再进上下文，引用出处
//! - `web_search` / `search_messages` / `search_docs` 工具（实现 gateway 里的
//!   `GatewayTool`，走 catalog）；跨群结果按请求者成员资格过滤，外部群默认不开
//!
//! 依赖为什么预声明：`aite-gateway` 是为了实现 `GatewayTool`（原卡定死）；
//! `reqwest` 发搜索请求；其余理由见本 crate 的 `Cargo.toml`。
