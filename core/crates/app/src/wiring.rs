//! 把评测接到真实现上（`aite evals` 的 `Wiring` 四个工厂）。owner: RΩ
//!
//! R7 把四个注入点留好了，这里填满：
//!
//! | 工厂 | 填的是什么 |
//! |---|---|
//! | `plane` | 真 `InProcessControlPlane`（R4）+ 真 `AgentWorker`（R5），其余零件用 Deps 里的替身 |
//! | `model` | `--model live`：`OpenAiCompatModel`（R5）。**起飞前先造一次**，见 [`live_model_factory`] |
//! | `sandbox` | `--sandbox docker`：edge（Go）的真沙箱 + 真 `P0ToolGateway`（R6），各套一层 R7 的探针 |
//! | `preflight` | 同一条 edge 连接做起飞前体检：版本、daemon、镜像真起一个容器 |
//!
//! **为什么 core 这侧要连 edge 才能跑 docker 档**：真沙箱在 Go 那边（spec §1 的语言分工），
//! core 自己不认识 Docker。所以 `--sandbox docker` 的前提是 `aite-edge` 在跑 ——
//! 它不需要飞书凭证（`platform: fake` 就够），但 socket 得通。
//!
//! **为什么要自己 peek argv**：`Wiring` 是在 `cli::run_with_wiring` 之前就得装好的，而
//! `--config` / `--model` / `--sandbox` 要到 `cli` 里才被解析（R7 审核记的那条：
//! `ModelFactory` 是零参数的，接线方只能闭包捕获）。这里 peek 的三个开关与 `cli` 的
//! 解析口径一致（`--x VALUE` 两段式），拿不准的一律退回默认，真正的校验还是 `cli` 做。
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use aite_contracts::{
    CONTRACT_VERSION, ControlPlane, EvidenceWriter, ModelPort, PlatformPort, SandboxPort,
    SessionStore, TaskWorker, ToolGateway,
};
use aite_control::{ControlDeps, InProcessControlPlane};
use aite_edge_client::EdgeClient;
use aite_evals::cli::Wiring;
use aite_evals::deps::{Deps, GatewayFacade, ModelFactory, SandboxFacade, SandboxFactory};
use aite_evals::real_stack::{DockerProbe, GatewayProbe, SandboxProbe};
use aite_evals::runner::PlaneFactory;
use aite_gateway::P0ToolGateway;
use aite_models::OpenAiCompatModel;
use aite_worker::{AgentWorker, WorkerDeps};
use tokio::runtime::Runtime;

use crate::app::{DEFAULT_CONFIG_PATH, load_config, sandbox_spec_of};

/// 体检时那个临时容器的 task_id。挑一个一眼看得出来的，真漏了也好 `docker ps` 里找。
const PREFLIGHT_TASK_ID: &str = "aite-preflight";

/// 真 ControlPlane + 真 AgentWorker，其余零件用场景给的替身。
///
/// 只有这一个工厂是无条件注入的 —— B8（`--platform fake --model scripted --sandbox fake`）
/// 走的就是它，`passed 10/10` 全靠它把 R4 与 R5 接进来。
///
/// **里面不许 panic**：runner 有 `catch_unwind` 兜底会收成 `phase="error"`，
/// 但那是安全网不是许可证（审核记账第 8 条）。所以构造路径上一个 `unwrap` 都没有。
pub fn plane_factory() -> PlaneFactory {
    Arc::new(|deps: &Deps| {
        let store: Arc<dyn SessionStore> = deps.store.clone();
        let platform: Arc<dyn PlatformPort> = deps.platform.clone();
        let evidence: Arc<dyn EvidenceWriter> = deps.evidence.clone();
        let model: Arc<dyn ModelPort> = deps.model.clone();
        // 两个 facade 都是对应契约 trait 的 subtrait，直接向上转型。
        let sandbox: Arc<dyn SandboxPort> = deps.sandbox.clone();
        let gateway: Arc<dyn ToolGateway> = deps.gateway.clone();

        let worker: Arc<dyn TaskWorker> = Arc::new(AgentWorker::new(WorkerDeps {
            store: store.clone(),
            platform: platform.clone(),
            model: model.clone(),
            evidence: evidence.clone(),
            config: deps.config.clone(),
            gateway: Some(gateway.clone()),
            sandbox: Some(sandbox.clone()),
        }));

        let plane = InProcessControlPlane::new(ControlDeps {
            store,
            platform,
            evidence,
            config: deps.config.clone(),
            worker: Some(worker),
            gateway: Some(gateway),
            sandbox: Some(sandbox),
            model_name: model.name(),
        });
        Ok(Arc::new(plane) as Arc<dyn ControlPlane>)
    })
}

/// `aite evals` 的全套接线。按 argv 决定要不要连 edge、要不要造真模型。
///
/// 返回 `Err` = 一行人话 + 退出码 2（和 `cli` 自己的「起不来」一个口径）。
pub fn evals_wiring(argv: &[String]) -> Result<Wiring, String> {
    let mut wiring = Wiring {
        plane: Some(plane_factory()),
        ..Wiring::default()
    };

    let config_path =
        peek_value(argv, "--config").unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_string());
    let want_live = peek_value(argv, "--model").as_deref() == Some("live");
    let want_docker = peek_value(argv, "--sandbox").as_deref() == Some("docker");

    if want_live {
        wiring.model = Some(live_model_factory(&config_path)?);
    }
    if want_docker {
        let config = load_config(&config_path).map_err(|e| {
            format!("--sandbox docker 起不来：{e}（真沙箱在 edge，要读 config 的 edge: 段）")
        })?;
        let repo_root = std::env::current_dir()
            .map_err(|e| format!("--sandbox docker 起不来：取不到当前工作目录：{e}"))?;
        let rt = edge_runtime()?;
        let edge = rt
            .block_on(EdgeClient::connect(&config.edge, &repo_root))
            .map_err(|e| {
                format!(
                    "--sandbox docker 起不来：连不上 edge（{}）：{e}",
                    repo_root.join(&config.edge.edge_socket).display()
                )
            })?;
        let edge = Arc::new(edge);
        // 成对注入：只给工厂不给体检会被 cli 直接拒（退出码 2，审核记账第 23 条）。
        wiring.sandbox = Some(docker_sandbox_factory(edge.clone()));
        wiring.preflight = Some(docker_probe(edge, rt));
    }
    Ok(wiring)
}

/// `--model live`：起飞前先把客户端造出来验一遍配置。
///
/// 不在这里拦的话，配置缺一样就会变成：每个场景各自跑到第一次 chat 才抛，被 worker 的
/// §3.3 当成模型 5xx **白重试 2 次（2s + 5s）**，最后给用户一句「模型服务暂不可用」。
/// 十个场景就是十次 7 秒空等，而真正的原因（yaml 没填 / 环境变量没设）一个字都看不到。
/// 造客户端不发网络请求，所以这里只判配置、不判端点通不通（那是 `aite preflight` 的活）。
fn live_model_factory(config_path: &str) -> Result<ModelFactory, String> {
    let config = load_config(config_path).map_err(|e| format!("--model live 起不来：{e}"))?;
    let env = aite_models::env_snapshot();
    // 试造一次，失败就地报。
    OpenAiCompatModel::from_config(&config.model, &env)
        .map_err(|e| format!("--model live 起不来：{e}（配置：{config_path}）"))?;
    let model_cfg = config.model.clone();
    // 每个场景造一个新的 —— 观测（累计 token / 花费）要按场景切开。
    Ok(Arc::new(move || {
        let env = aite_models::env_snapshot();
        OpenAiCompatModel::from_config(&model_cfg, &env)
            .map(|m| Arc::new(m) as Arc<dyn ModelPort>)
            .map_err(|e| e.to_string())
    }))
}

/// `--sandbox docker`：edge 的真沙箱 + 真 `P0ToolGateway`，各套一层 R7 的探针。
///
/// 平台仍然是 `FakePlatform`（附件从它来、回执也发回它）—— 这一档验的是沙箱与 Gateway
/// 这一段真的走通了。`resolver` 是 R7 递过来的现成货：`P0ToolGateway` 的令牌校验是
/// 失败关闭的，不接就是每个工具调用都 `denied`，而报出来的是「denied」不是「你没接」。
fn docker_sandbox_factory(edge: Arc<EdgeClient>) -> SandboxFactory {
    Arc::new(move |platform, _store, config, resolver| {
        let probe = Arc::new(SandboxProbe::new(edge.sandbox()));
        let sandbox: Arc<dyn SandboxFacade> = probe.clone();
        // Gateway 拿的是探针而不是裸 Port：run_python 走的那几发 exec/put/get 也要记账，
        // 不然 `check: sandbox_calls` 在这一档下全是 0。
        let inner: Arc<dyn SandboxPort> = probe;
        let gw = P0ToolGateway::new(platform.clone(), inner, sandbox_spec_of(config))
            .with_token_resolver(resolver.clone());
        let gateway: Arc<dyn GatewayFacade> = Arc::new(GatewayProbe::new(Arc::new(gw)));
        Ok((sandbox, gateway))
    })
}

/// 起飞前体检：edge 通不通、契约版本对不对、daemon 在不在、镜像起不起得来。
///
/// 最后一条是真起一个容器（`acquire` 自带 `_check_ready`，不过就地 release），
/// 所以它同时覆盖了旧 `preflight.py` 第 6 项的「镜像存在」与「四个 import 能跑」。
/// **一定要 release**：否则体检自己留一个容器，验收那条 `docker ps -a --filter
/// label=aite.task` 就不是 0 了。
fn docker_probe(edge: Arc<EdgeClient>, rt: &'static Runtime) -> DockerProbe {
    Arc::new(move |images: &[String]| {
        let edge = edge.clone();
        let mut wanted: Vec<String> = images.to_vec();
        wanted.sort();
        wanted.dedup();
        rt.block_on(async move {
            let status = match edge.status().await {
                Ok(s) => s,
                Err(e) => {
                    return Some(format!(
                        "连不上 edge（{}）：{e}。先起 `aite-edge --config <配置>`（不需要飞书凭证，platform: fake 也行）",
                        edge.edge_socket().display()
                    ));
                }
            };
            if status.contract_version != CONTRACT_VERSION {
                return Some(format!(
                    "两边契约版本不一致：core {CONTRACT_VERSION} vs edge {}（edge 版本 {}）",
                    status.contract_version, status.version
                ));
            }
            if !status.sandbox_ok {
                return Some(
                    "edge 连得上，但它报 docker daemon 不可达（EdgeStatus.sandbox_ok=false）：\
                     先 `docker info` 确认 daemon 在跑，且 edge 摸得到 /var/run/docker.sock"
                        .to_string(),
                );
            }
            let sandbox = edge.sandbox();
            for image in wanted {
                let mut spec = aite_contracts::SandboxSpec::new(&image);
                spec.network = aite_contracts::SandboxNetwork::None;
                match sandbox.acquire(PREFLIGHT_TASK_ID, &spec).await {
                    Ok(id) => {
                        // 体检不留尾巴。release 是幂等的，失败也只是说一句。
                        if let Err(e) = sandbox.release(&id).await {
                            tracing::warn!(target: "aite.preflight", sandbox = %id, error = %e, "preflight.release_failed");
                        }
                    }
                    Err(e) => {
                        return Some(format!(
                            "镜像 {image} 起不来容器：{e}。先 `docker build -t {image} docker/sandbox`"
                        ));
                    }
                }
            }
            None
        })
    })
}

/// 给 edge 连接用的专属 runtime。
///
/// 为什么要单独一条：`EdgeClient::connect` 里的 `connect_lazy()` 会 `tokio::spawn` 一条
/// channel worker，必须在 runtime 上下文里建；而 `cli::run_with_wiring` 自己要建一个
/// current_thread runtime 跑场景（`hold_ticks` 的让出语义依赖单事件循环），从里面再
/// `block_on` 会 panic。于是：连接与 channel/hyper 的那些后台 task 跑在这条专属
/// runtime 上（它有自己的线程），场景那条 runtime 只负责 await 结果。
/// 它活到进程结束 —— 提前 drop 会把 channel worker 一起带走。
fn edge_runtime() -> Result<&'static Runtime, String> {
    static RT: OnceLock<Runtime> = OnceLock::new();
    if let Some(rt) = RT.get() {
        return Ok(rt);
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .thread_name("aite-edge-link")
        .build()
        .map_err(|e| format!("edge 连接用的 runtime 起不来：{e}"))?;
    Ok(RT.get_or_init(|| rt))
}

/// `--flag VALUE` 两段式取值；同名多次取最后一次（与 `cli::parse_args` 的覆盖口径一致）。
fn peek_value(argv: &[String], flag: &str) -> Option<String> {
    let mut found = None;
    let mut i = 0;
    while i < argv.len() {
        if argv[i] == flag {
            found = argv.get(i + 1).cloned();
            i += 2;
        } else {
            i += 1;
        }
    }
    found
}

/// 仓库根（socket 与 storage 的相对路径都落在这里）。
pub fn repo_root_or_cwd(explicit: Option<&Path>) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}
