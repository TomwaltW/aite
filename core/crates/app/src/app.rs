//! 组装面（对应旧 `aite/app.py` 的 `build_app` 那一段 + `aite/config.py`）。
//!
//! 顺序有讲究，照 `review/inventory-core.md` §7 的 11 步。与 Python 的差异只有一处：
//! 第 3/4 步。Python 版单进程，`_build_platform` 直接 new 一个 `FeishuPlatform`、
//! `_build_sandbox` 直接 new 一个 `DockerSandbox`；这一版两样都在 edge（Go）进程里，
//! core 侧换成 `EdgeClient::connect` + 一次 `contract_version` 比对（spec §2.1）。
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aite_contracts::{
    AiteConfig, CONTRACT_VERSION, ControlPlane, EvidenceWriter, ModelPort, PlatformChoice,
    PlatformPort, SandboxNetwork, SandboxPort, SandboxSpec, SessionStore, StorageConfig,
    TaskWorker, ToolGateway,
};
use aite_control::{ControlDeps, InProcessControlPlane, Ingress};
use aite_edge_client::EdgeClient;
use aite_evidence::FileEvidenceWriter;
use aite_gateway::P0ToolGateway;
use aite_models::OpenAiCompatModel;
use aite_store::SqliteSessionStore;
use aite_worker::{AgentWorker, WorkerDeps, context::load_system_prompt};

use crate::features::{self, FeatureCtx};

/// 起飞前体检没过 / 配置读不到时的退出码。跟评测 runner 的「起不来」一个口径。
pub const EXIT_STARTUP: i32 = 2;
/// 第二次收到信号硬退时的退出码。
pub const EXIT_HARD_STOP: i32 = 130;

/// 默认配置路径（对应 Python 的 `DEFAULT_CONFIG_PATH`）。
pub const DEFAULT_CONFIG_PATH: &str = "config/aite.yaml";

/// 起飞时向 edge 问 `GetStatus` 的次数，每次间隔 1s。
///
/// §2.1 要的是两件看着矛盾的事：「启动顺序无关，连不上不退出」和「`contract_version`
/// 不等就拒绝起飞」。只拨一次的话 `docker compose up` 下 core 多半比 edge 快，版本门禁
/// 就成了摆设；无限等又违反前一条。折中：最多 5 次 × 1s —— 答上来就严格比对，
/// 始终答不上来就记一行 `aite.edge_unreachable` 照常起飞（懒连接会自己恢复）。
///
/// **这里只管「起飞这一次」。** 起飞之后的复查归 `aite-edge-client` 的契约闸门
/// （那个 crate 的 `gate.rs`）：后台探针每重新连上一次就比一次，比出不一致就闸断
/// platform / sandbox 两条链路、对上了自己解开。没有它的话「4s 内没拨通就放行」
/// 等于此后整个进程生命周期都不再比对 —— 而 M6 的「只重启 edge」那一遍走的正是这条路。
const EDGE_STATUS_ATTEMPTS: u32 = 5;

/// 起飞前就能看出来的问题。消息本身就是给人看的那句话，`cli` 直接原样打出去。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct StartupError(pub String);

impl StartupError {
    fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// 三个可注入的口子 + 仓库根。给了就用给的，一个字都不查（替身不需要凭证）。
#[derive(Default, Clone)]
pub struct Injections {
    pub platform: Option<Arc<dyn PlatformPort>>,
    pub model: Option<Arc<dyn ModelPort>>,
    pub sandbox: Option<Arc<dyn SandboxPort>>,
    /// `edge:` 段里那两个相对路径落到哪棵树上；默认进程的工作目录。
    pub repo_root: Option<PathBuf>,
}

/// 装好的一整套。字段名与 Python 版 `AiteApp` 对齐，集成测试按名字取件。
pub struct AiteApp {
    pub config: AiteConfig,
    pub platform: Arc<dyn PlatformPort>,
    pub store: Arc<dyn SessionStore>,
    pub evidence: Arc<dyn EvidenceWriter>,
    pub sandbox: Option<Arc<dyn SandboxPort>>,
    pub gateway: Option<Arc<dyn ToolGateway>>,
    pub model: Arc<dyn ModelPort>,
    pub worker: Arc<dyn TaskWorker>,
    pub plane: Arc<dyn ControlPlane>,
    pub ingress: Ingress,
    /// 真机那一路才有：`platform` / `sandbox` 有一个没被注入就得连 edge，连出来的客户端
    /// 挂在这儿；三个口子全注入 → `None`（`build_app_contract.rs` 钉着这条分界）。
    ///
    /// **产品代码里只有两处读它**（测试另算）：`run.rs` 的 `log_takeoff` 印起飞那行 `aite.up` 里的 edge socket
    /// 绝对路径（真机排障的第一现场），以及本文件 `Debug` 里那个 `edge: true/false`。
    /// 原注释写的「`!status` 的健康行与收尾时的连接态日志」**两处都不存在**：
    /// `cmd_status`（`control/src/plane.rs`）从头到尾不碰 edge，收尾路径也没有任何一行
    /// 连接态日志 —— 唯一那行是起飞时打的。
    ///
    /// **别把它当生命线。** `EdgeClient` 交出去的 `platform()` / `sandbox()` 各自攥着同一根
    /// `Link` 的 `Arc`，后台重连探针又是 `tokio::spawn` 出去、自己也攥着一个 —— 所以 V5 之后
    /// edge 客户端真正要紧的那件事，契约闸门（`edge-client/src/gate.rs`：每重新连上一次就补比
    /// 一次 `contract_version`，不一致就闸断 platform / sandbox 两条链路），**不经过这个字段**，
    /// 它置空也照跑。
    pub edge: Option<Arc<EdgeClient>>,
}

/// 手写而不是 derive：十个字段里八个是 `Arc<dyn Trait>`，没有 Debug 可用。
/// 存在的理由是测试里 `expect_err` 这类断言要能把「本该失败却装成了」打出来，
/// 以及排障时想知道「到底装了谁」。**绝不打配置里的取值之外的东西**（密钥只在环境变量里，
/// 这里印的全是形状）。
impl std::fmt::Debug for AiteApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiteApp")
            .field("platform", &self.config.platform.as_str())
            .field("model", &self.model.name())
            .field("sandbox", &self.sandbox.is_some())
            .field("gateway", &self.gateway.is_some())
            .field("edge", &self.edge.is_some())
            .field("sqlite_path", &self.config.storage.sqlite_path)
            .field("evidence_dir", &self.config.storage.evidence_dir)
            .finish()
    }
}

/// 读 `config/aite.yaml` → `AiteConfig`（对应 Python 的 `load_config`）。
///
/// 文件缺字段用契约默认值；空文件 = 全默认；顶层不是 mapping 或有未知键都报错
/// （`deny_unknown_fields`，D6：防拼错静默失效）。
pub fn load_config(path: impl AsRef<Path>) -> Result<AiteConfig, StartupError> {
    let p = path.as_ref();
    if !p.exists() {
        return Err(StartupError::new(format!(
            "配置文件不存在：{}（可从 config/aite.example.yaml 复制）",
            p.display()
        )));
    }
    let text = std::fs::read_to_string(p)
        .map_err(|e| StartupError::new(format!("配置文件读不出来：{}：{e}", p.display())))?;
    AiteConfig::from_yaml_str(&text)
        .map_err(|e| StartupError::new(format!("配置文件读不懂：{}：{e}", p.display())))
}

/// 跟评测那一档同一口径 —— 真机怎么从 config 出 spec，评测就怎么出。
pub fn sandbox_spec_of(config: &AiteConfig) -> SandboxSpec {
    SandboxSpec {
        image: config.sandbox.image.clone(),
        cpu: config.sandbox.cpu,
        mem_mb: config.sandbox.mem_mb,
        network: SandboxNetwork::None,
        workdir: "/work".to_string(),
    }
}

/// 按 config 把零件装起来。
///
/// **只组装**：不起容器、不发消息、不建表（建表是起飞时的 `store.init()`）、不处理事件。
/// 但原来那句「不连网，唯一的副作用是建那三个落盘目录」是假的 —— C-TΩ-1 的边界实际画在
/// 下面这几条上，逐条写清楚，别让人照着那句话去推断。
///
/// **落盘**
/// - `prepare_storage`：`mkdir -p` 三个目录（sqlite 的父目录、`evidence_dir`、
///   `artifacts_dir`）。这是硬约束 1 明文允许的那一条，也是 `aite preflight` 第 7 项
///   对人承诺的「data 待建，起飞时自动 mkdir」。
/// - `SqliteSessionStore::open`：打开连接，**库文件不在就建一个空的**（里面还没有表）。
///   反过来，**文件在、但根本不是个库**时这一步照样成功 —— SQLite 是懒打开的，
///   要到第一次真去读库头才认出来，也就是起飞时 `run.rs` 的 `store.init()` 建表那一下：
///   那时才炸，退出码 2、`aite 起不来：建表失败（…）`。所以这条边界不在 `build_app` 上。
///   （目录、坏符号链接、没读权限这几种是例外，`Connection::open` 当场就打不开。）
///   提前把它验出来的是 `aite preflight` 第 1 组第 4 件事（`preflight.rs` 的 `sqlite_fault`，
///   拿 `PRAGMA schema_version` 探一下就还回去）—— 口径以 `preflight.rs` 模块头那张表为准，
///   那里是唯一把这四件事写全的地方。
///
/// **读盘**：`require_system_prompt` 真去读 `worker.system_prompt_path`，读不到拒绝起飞。
///
/// **连 edge**：只有 `platform` / `sandbox` 至少缺一个注入时才走（`need_edge`）。
/// 三个口子全注入就完全不碰 edge —— 这条分界由 `build_app_contract.rs` 两条测试钉着。
/// 走上去的话是两件事，代价差得很远：
/// - `EdgeClient::connect` 是**懒连接，不发一个字节**：解析 socket 路径、备一条 tonic 的
///   lazy Channel，再 `tokio::spawn` 一条后台重连探针（1→2→…→30s 退避）。
///   它只会因为「socket 路径拼不成合法地址」失败。
/// - `check_contract_version` 才是真 RPC：`GetStatus` 最多 `EDGE_STATUS_ATTEMPTS`（5）次、
///   每两次之间 `sleep(1s)`。所以 **edge 没起来时最坏在这里阻塞约 4s**，
///   然后记一行 `aite.edge_unreachable` 照常返回 `Ok`（§2.1「启动顺序无关」）；
///   答上来而版本不一致则是 `StartupError`，拒绝起飞。
///
/// **edge 完全没起时组装照样成功，而且起飞会一路走到 `serve()`** —— 这是既有行为、
/// 是刻意的，不是漏判。整条路上只有 `platform.start()` 里的 `ingress.start()`（core 自己
/// 监听 `core_socket`）是硬要求；能力表问不到只是一行 `edge.capabilities_unavailable` 的
/// warn，上面那个 `check_contract_version` 等满 5 次也照常返回 `Ok`。依据是 §2.1
/// 「启动顺序无关，连不上不退出」：两边都是懒连接 + 退避重连，edge 后起时探针拨通那一下
/// 会补比一次版本（`edge-client/src/gate.rs` 的契约闸门）。
/// 钉它的是 `tests/signals.rs::edge_absent_still_takes_off_to_ingress_listening`；
/// 同一个文件里那条进程级停机回归（`sigterm_before_serve_still_exits_by_itself`）
/// **就建在这条行为上** —— 哪天有人把「edge 不可达就拒绝起飞」当成改进加进来，
/// 那条会莫名其妙地红，而红的理由跟它要测的事（信号）毫无关系。先看这一条。
pub async fn build_app(
    config: AiteConfig,
    inject: Injections,
) -> Result<Box<AiteApp>, StartupError> {
    build_app_with_features(config, inject, |_| Ok(())).await
}

/// [`build_app`] 加上功能接缝（CC4 ⑥）。`build_app` = 它 + `|_| Ok(())`。
///
/// 第 7 步（证据）之后、第 8 步（Gateway）之前：`features::wire_all` 先跑，`extra` 在它**之后**跑
/// （测试往同一组有序槽里追加）；两者的 `Err` 都在这里**一次**映射成 `StartupError`。
/// 第 1–7 步的先后一步都不动（拒绝起飞时先报哪条错是行为）。
///
/// 应用顺序：模型覆盖（**注入的 model 仍优先**：给了就用给的；覆盖只替掉按 config 造的那个，
/// 同一个 model 一路给到 worker、`ControlDeps.model_name`、`AiteApp.model`）→ `P0ToolGateway::new`
/// → 登记 → `gateway_options` 依次 → 包 `Arc` → `WorkerDeps` → `worker_options` 依次 → `AgentWorker::new`。
/// 全程纯内存（`build_app_contract.rs` 的 2s 上限钉着），不做 I/O。
pub async fn build_app_with_features(
    config: AiteConfig,
    inject: Injections,
    extra: impl FnOnce(&mut FeatureCtx) -> Result<(), String>,
) -> Result<Box<AiteApp>, StartupError> {
    // 1 建落盘目录
    prepare_storage(&config.storage)?;
    // 2 system prompt 必须读得到（W9 那四条铁律在 platform.md 里，缺了每个任务都会在第一步炸）
    require_system_prompt(&config)?;

    // 3 真平台与真沙箱都在 edge（Go）。两个口子都被注入时根本不需要 edge。
    let repo_root = match inject.repo_root.clone() {
        Some(p) => p,
        None => std::env::current_dir()
            .map_err(|e| StartupError::new(format!("取不到当前工作目录：{e}")))?,
    };
    let need_edge = inject.platform.is_none() || inject.sandbox.is_none();
    if need_edge && inject.platform.is_none() && config.platform == PlatformChoice::Fake {
        return Err(StartupError::new(
            "config.platform=fake 时必须由调用方注入平台实现（build_app 的 Injections.platform）。\
             fake 是留给评测 / 回放的取值（§3.1），不会去连真实飞书；\
             真机起飞请把 config/aite.yaml 的 platform 改成 feishu。",
        ));
    }
    let edge = if need_edge {
        let client = EdgeClient::connect(&config.edge, &repo_root)
            .await
            .map_err(|e| {
                StartupError::new(format!(
                    "连不上 edge（{}）：{}。socket 路径由 config 的 edge.edge_socket 决定，\
                     相对仓库根 —— 多半是没在仓库根起进程，或者 aite-edge 还没起。",
                    repo_root.join(&config.edge.edge_socket).display(),
                    e
                ))
            })?;
        let client = Arc::new(client);
        // 4 比对 contract_version：不等就拒绝起飞，两边版本都印出来
        check_contract_version(&client).await?;
        Some(client)
    } else {
        None
    };

    let platform: Arc<dyn PlatformPort> = match inject.platform {
        Some(p) => p,
        // need_edge 为真时 edge 一定是 Some（上面那段唯一的出口是 return Err）
        None => edge.as_ref().expect("edge 已连上").platform(),
    };
    let sandbox: Arc<dyn SandboxPort> = match inject.sandbox {
        Some(s) => s,
        None => edge.as_ref().expect("edge 已连上").sandbox(),
    };

    // 5 模型
    let model_injected = inject.model.is_some();
    let model: Arc<dyn ModelPort> = match inject.model {
        Some(m) => m,
        None => Arc::new(build_model(&config)?),
    };

    // 6 SQLite
    let store: Arc<dyn SessionStore> = Arc::new(
        SqliteSessionStore::open(&config.storage.sqlite_path).map_err(|e| {
            StartupError::new(format!(
                "打不开 SQLite（{}）：{e}",
                config.storage.sqlite_path
            ))
        })?,
    );
    // 7 证据
    let evidence: Arc<dyn EvidenceWriter> =
        Arc::new(FileEvidenceWriter::new(&config.storage.evidence_dir));

    // 7½ 功能接缝（CC4 ⑥）：组装前的 wire，纯内存
    let mut feats = features::wire_features(config.clone(), store.clone())
        .map_err(|e| StartupError::new(format!("功能接线失败：{e}")))?;
    extra(&mut feats).map_err(|e| StartupError::new(format!("功能接线失败：{e}")))?;
    let FeatureCtx {
        registry,
        model_override,
        gateway_options,
        worker_options,
        ..
    } = feats;
    let model: Arc<dyn ModelPort> = match model_override {
        Some(over) if !model_injected => over,
        _ => model,
    };

    // 8 Gateway。令牌校验是失败关闭的：`AgentWorker::run` 在任务真正开跑那一刻
    // `register_task(task.id, task.session_token)`（agent.rs:869，旧版 AppWorker 的活已
    // 并进 worker）。这里**不**接 `with_token_resolver` —— 它会整个替掉登记表，而
    // `TokenResolver` 是同步签名、`SessionStore::get_task` 是 async：真机上唯一的同步
    // 退路是在 async 里阻塞着读 SQLite（违反 §7.6），换来的只是一条本来就走得通的路。
    // 评测那一档不同：`FakeSessionStore` 有不记账的同步快照，所以 wiring 照 R7 的设计注入。
    let gateway: Arc<dyn ToolGateway> = Arc::new(FeatureCtx::build_gateway(
        registry,
        gateway_options,
        P0ToolGateway::new(platform.clone(), sandbox.clone(), sandbox_spec_of(&config)),
    ));

    // 9 worker
    let mut worker_deps = WorkerDeps {
        store: store.clone(),
        platform: platform.clone(),
        model: model.clone(),
        evidence: evidence.clone(),
        config: config.clone(),
        gateway: Some(gateway.clone()),
        sandbox: Some(sandbox.clone()),
    };
    features::apply_worker_options(&mut worker_deps, worker_options);
    let worker: Arc<dyn TaskWorker> = Arc::new(AgentWorker::new(worker_deps));

    // 10 控制面
    let plane: Arc<dyn ControlPlane> = Arc::new(InProcessControlPlane::new(ControlDeps {
        store: store.clone(),
        platform: platform.clone(),
        evidence: evidence.clone(),
        config: config.clone(),
        worker: Some(worker.clone()),
        gateway: Some(gateway.clone()),
        sandbox: Some(sandbox.clone()),
        model_name: model.name(),
    }));

    // 11 事件入口
    let ingress = Ingress::new(plane.clone());

    Ok(Box::new(AiteApp {
        config,
        platform,
        store,
        evidence,
        sandbox: Some(sandbox),
        gateway: Some(gateway),
        model,
        worker,
        plane,
        ingress,
        edge,
    }))
}

/// 建落盘目录。C-TΩ-1 硬约束 1 允许的唯一副作用。
///
/// `StorageConfig` 有几个路径就建几个，一个都不能漏 —— `aite preflight` 第 7 项对人
/// 承诺的原话是「data 待建，起飞时自动 mkdir」，目录不在它不算 FAIL，正是因为这里会建。
fn prepare_storage(cfg: &StorageConfig) -> Result<(), StartupError> {
    let mkdir = |p: &Path| -> Result<(), StartupError> {
        std::fs::create_dir_all(p)
            .map_err(|e| StartupError::new(format!("建不出目录 {}：{e}", p.display())))
    };
    if cfg.sqlite_path != ":memory:"
        && let Some(parent) = Path::new(&cfg.sqlite_path).parent()
        && !parent.as_os_str().is_empty()
    {
        mkdir(parent)?;
    }
    mkdir(Path::new(&cfg.evidence_dir))?;
    mkdir(Path::new(&cfg.artifacts_dir))?;
    Ok(())
}

/// system prompt 必须读得到，读不到就拒绝起飞。
///
/// 报错文案按**真实病史**排序，别把人带反：2026-09-12 总管撞上这一条时 cwd 就是仓库根，
/// 而原来那句话写的是「多半是没在仓库根起进程」—— 真因是他那份 2026-09-10 写的配置还
/// 指着当天被删掉的 Python 树。所以现在先说配置里的路径本身不对，再说 cwd 那一种，
/// 并且把**解析成的绝对路径**打出来，让人一眼看见它究竟去哪儿找了。
///
/// 同一条判据现在也在 `aite preflight` 第 1 组里（`preflight.rs` 的 `check_config`，
/// 复用的就是下面这个 [`load_system_prompt`]）—— 两边同一条路径，不会一个说行一个说不行。
fn require_system_prompt(config: &AiteConfig) -> Result<(), StartupError> {
    let raw = &config.worker.system_prompt_path;
    load_system_prompt(raw).map_err(|e| {
        let resolved = std::env::current_dir()
            .map(|cwd| cwd.join(raw).display().to_string())
            // cwd 都取不到时退回原值：这一行是给人拿去 `ls -l` 的，宁可少说也别编。
            .unwrap_or_else(|_| raw.clone());
        StartupError::new(format!(
            "读不到 system prompt：{e}。配置项是 worker.system_prompt_path（当前值 {raw}），\
             找的是 {resolved}。多半是配置里这一行本身不对 —— 2026-09-12 Python 树删了，\
             旧配置里的 aite/worker/prompts/ 已经不存在，该改成 \
             core/crates/worker/prompts/platform.md（config/aite.example.yaml 里就是这个值）；\
             路径是相对进程工作目录算的，所以也可能是没在仓库根起进程。\
             `aite preflight` 第 1 组现在会提前拦下这一条。"
        ))
    })?;
    Ok(())
}

fn build_model(config: &AiteConfig) -> Result<OpenAiCompatModel, StartupError> {
    let cfg = &config.model;
    if cfg.provider != aite_contracts::ModelProvider::OpenaiCompat {
        return Err(StartupError::new(format!(
            "config.model.provider={} 时必须由调用方注入模型实现（Injections.model）。\
             scripted 是留给评测的取值（§3.1）。",
            cfg.provider
        )));
    }
    // base_url / model / 密钥环境变量三样的人话在 `from_config` 里，原样转出来。
    OpenAiCompatModel::from_config(cfg, &aite_models::env_snapshot())
        .map_err(|e| StartupError::new(e.to_string()))
}

/// 两边契约版本必须一致，不等就拒绝起飞（§2.1）。
///
/// **答不上来不是拒绝起飞的理由** —— 5 次都拨不通就记一行 `aite.edge_unreachable`
/// 返回 `Ok`，起飞继续走完（`platform.start()` 里只有本地那个 `ingress.start()` 是硬要求），
/// 依据同样是 §2.1「启动顺序无关，连不上不退出」。这条行为被
/// `tests/signals.rs::edge_absent_still_takes_off_to_ingress_listening` 钉着，
/// 那个文件里的进程级停机回归也建在它上面 —— 详见 [`build_app`] 文档注释末段。
async fn check_contract_version(edge: &Arc<EdgeClient>) -> Result<(), StartupError> {
    let mut last: Option<String> = None;
    for attempt in 1..=EDGE_STATUS_ATTEMPTS {
        match edge.status().await {
            Ok(status) => {
                if status.contract_version != CONTRACT_VERSION {
                    return Err(StartupError::new(format!(
                        "两边契约版本不一致，拒绝起飞：core CONTRACT_VERSION={CONTRACT_VERSION}，\
                         edge contract_version={}（edge 版本 {}）。\
                         两个进程要一起升 —— 重新 `cargo build` + `go build` 之后再起。",
                        status.contract_version, status.version
                    )));
                }
                tracing::info!(
                    target: "aite.app",
                    edge_version = %status.version,
                    contract_version = %status.contract_version,
                    edge_platform = %status.platform,
                    platform_connected = status.platform_connected,
                    sandbox_ok = status.sandbox_ok,
                    "aite.edge_status"
                );
                return Ok(());
            }
            Err(e) => {
                last = Some(e.to_string());
                if attempt < EDGE_STATUS_ATTEMPTS {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
    }
    // §2.1「启动顺序无关」：edge 还没起不是起飞失败。**起飞这一轮**的版本门禁确实没生效，
    // 所以要在日志里说出来，别让人以为比过了 —— 但这不再是个缺口：edge 一起来，
    // edge-client 的后台探针拨通那一下就会补比一次（`gate.rs` 的契约闸门），
    // 不一致就把链路闸断并说明怎么恢复。
    tracing::warn!(
        target: "aite.app",
        socket = %edge.edge_socket().display(),
        attempts = EDGE_STATUS_ATTEMPTS,
        error = %last.unwrap_or_default(),
        "aite.edge_unreachable contract_version 起飞这一轮没比成；edge 起来后探针会补比一次"
    );
    Ok(())
}

/// `session.config_snapshot["initiator_name"]` → 卡片上的发起人显示名。
///
/// 与 `aite_control::plane::initiator_of` 逐字同义（那个是私有的，照抄比导出便宜）。
pub(crate) fn initiator_of(session: &aite_contracts::Session) -> String {
    session
        .config_snapshot
        .get("initiator_name")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| session.created_by.clone())
}

/// 环境变量快照（`aite_models::env_snapshot` 的别名，preflight 也用）。
pub fn env_snapshot() -> HashMap<String, String> {
    aite_models::env_snapshot()
}
