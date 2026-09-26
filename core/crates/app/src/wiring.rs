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
//!
//! **为什么两个档都是惰性的**：peek 出来的开关只够决定「装不装这条线」，装的时候
//! **一件 I/O 都不许做**。做了就等于把配置校验挪到了 `cli::parse_args` 前面 —— 一台没配好
//! 模型的机器上，`--list` 列不出场景名、`-h` 打不出用法、连「`--platform` 拼错了」这种纯
//! 参数错误都会被一句「`--model live` 起不来」抢先挡掉（RΩ 刚把 `evals run -h` 修成
//! stdout + 退出码 0，命令行上多一个 `--model live` 就当场失效，而唯一钉住它的测试走的是
//! `run_capture` 直调、看不见 `main.rs` 这一层）。
//!
//! 所以 `load_config` / `EdgeClient::connect` / `OpenAiCompatModel::from_config` 全部推迟到
//! 工厂**第一次被调用**时，而调用点正好落在 Python `aite/evals/__main__.py` 原来的位置上：
//!
//! ```text
//! parse → load_suite → --only → --list(return 0) → docker 体检 → scale → live 试造
//!                                     ↑ EdgeClient::connect        ↑ load_config + from_config
//! ```
//!
//! 顺序因此是被**代码结构**钉住的 —— 不是被一张「哪些参数该短路」的白名单钉住的
//! （那种白名单永远补不齐：`--only` 写错、suite 路径写错、`--platform` 拼错都不在名单上）。
//! 两条不许退回去的约束：模型客户端**仍然**在跑场景之前试造一次（见
//! [`live_model_factory`]），`preflight` 与 `sandbox` **仍然**成对注入（见 [`evals_wiring`]）。
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use aite_contracts::{
    CONTRACT_VERSION, ControlPlane, EvidenceWriter, ExecRequest, ExecResult, ModelPort,
    PlatformPort, SandboxError, SandboxPort, SandboxSpec, SessionStore, TaskWorker, ToolGateway,
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
use async_trait::async_trait;
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

        // CC4 ⑦：评测这一路也走同一个接缝（与 `build_app_with_features` 共用 features 的帮助函数）。
        // 只接 worker 选项：模型与网关是场景给的替身、是断言面，模型覆盖与 gateway 槽在这里不接。
        let feats = crate::features::wire_features(deps.config.clone(), store.clone())?;
        let mut worker_deps = WorkerDeps {
            store: store.clone(),
            platform: platform.clone(),
            model: model.clone(),
            evidence: evidence.clone(),
            config: deps.config.clone(),
            gateway: Some(gateway.clone()),
            sandbox: Some(sandbox.clone()),
        };
        crate::features::apply_worker_options(&mut worker_deps, feats.worker_options);
        let worker: Arc<dyn TaskWorker> = Arc::new(AgentWorker::new(worker_deps));

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

/// `aite evals` 的全套接线。按 argv 决定要不要装这两条线。
///
/// **这里一件 I/O 都不做**（模块头「为什么两个档都是惰性的」）：装的是闭包，配置与连接
/// 推迟到工厂第一次被调用时。签名保留 `Result` 是为了不动 `main.rs`（归 R0，`main.rs:1–2`
/// 写着「各轨落地时不需要动这个文件」）—— 它现在恒为 `Ok`，真正的「起不来」由
/// `evals::cli` 在 parse 之后的正确位置报。
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
        wiring.model = Some(live_model_factory(config_path.clone()));
    }
    if want_docker {
        let edge = Arc::new(LazyEdge::new(config_path));
        // 成对注入：只给工厂不给体检会被 cli 直接拒（退出码 2，审核记账第 23 条）。
        // 惰性之后这条更要紧了 —— 体检那一步同时是 edge 连接真正发生的地方，
        // 缺了它，沙箱工厂会在场景里拿到一条没连过的线（`LazyEdge::connected` 的那句人话）。
        wiring.sandbox = Some(docker_sandbox_factory(edge.clone()));
        wiring.preflight = Some(docker_probe(edge));
    }
    Ok(wiring)
}

/// edge 连接的惰性持有者：`--sandbox docker` 那一档的 `load_config` 与
/// `EdgeClient::connect` 都推迟到这里，第一次真要用的时候才发生。
///
/// 失败也进 `OnceLock`：十个场景不该各自重试一遍连不上的 socket，报出来的也该是同一句话。
struct LazyEdge {
    config_path: String,
    cell: OnceLock<Result<Arc<EdgeClient>, String>>,
}

impl LazyEdge {
    fn new(config_path: String) -> Self {
        Self {
            config_path,
            cell: OnceLock::new(),
        }
    }

    /// 连上（或复用已经连上的那条）。
    ///
    /// **只能从没有 tokio 上下文的线程调** —— 它要 `block_on`。唯一的调用点是起飞前体检
    /// （`evals::cli` 的 `docker_preflight`），那时场景那条 current_thread runtime 还没建。
    fn connect(&self) -> Result<Arc<EdgeClient>, String> {
        self.cell.get_or_init(|| self.do_connect()).clone()
    }

    /// 拿已经连上的那条。**绝不在这里 `block_on`**：沙箱工厂是在场景 runtime 里被调的，
    /// 从那儿 `block_on` 另一条 runtime 会 panic（纪律 4）。没连过 = 体检没跑 = 接线漏了
    /// 成对注入，报一行人话让它收成 `phase="error"`，而不是炸掉整套评测。
    fn connected(&self) -> Result<Arc<EdgeClient>, String> {
        match self.cell.get() {
            Some(r) => r.clone(),
            None => Err("--sandbox docker：edge 还没连上（起飞前体检没跑过）。\
                         preflight 与 sandbox 要成对注入（Wiring::preflight）。"
                .to_string()),
        }
    }

    fn do_connect(&self) -> Result<Arc<EdgeClient>, String> {
        let config = load_config(&self.config_path)
            .map_err(|e| format!("{e}（真沙箱在 edge，要读 config 的 edge: 段）"))?;
        let repo_root = std::env::current_dir().map_err(|e| format!("取不到当前工作目录：{e}"))?;
        let rt = edge_runtime()?;
        let edge = rt
            .block_on(EdgeClient::connect(&config.edge, &repo_root))
            .map_err(|e| {
                format!(
                    "连不上 edge（{}）：{e}",
                    repo_root.join(&config.edge.edge_socket).display()
                )
            })?;
        Ok(Arc::new(edge))
    }
}

/// `--model live`：造真模型客户端的工厂。**读配置也推迟到第一次调用**。
///
/// **「起飞前试造一次」这条没丢，只是换了地方**：现在由 `evals::cli` 在跑场景之前调一次
/// 并把结果丢掉（位置对齐 Python `__main__.py` 的 `_live_model_factory`，排在 parse /
/// load_suite / `--only` / `--list` / docker 体检 / scale **之后**）。
///
/// 不试造的话，配置缺一样就会变成：每个场景各自跑到第一次 chat 才抛，被 worker 的
/// §3.3 当成模型 5xx **白重试 2 次（2s + 5s）**，最后给用户一句「模型服务暂不可用」。
/// 十个场景就是十次 7 秒空等，而真正的原因（yaml 没填 / 环境变量没设）一个字都看不到。
/// 造客户端不发网络请求，所以这里只判配置、不判端点通不通（那是 `aite preflight` 的活）。
///
/// 错误串**不带** `--model live 起不来：` 前缀 —— 那是调用方加的，在这里加就会叠成
/// 「起不来：起不来：」。也**不带任何取值**：只有配置文件路径（纪律 6）。
fn live_model_factory(config_path: String) -> ModelFactory {
    // 配置只读一次盘（十个场景 + 那次试造共用），但客户端每个场景造一个新的 ——
    // 观测（累计 token / 花费）要按场景切开。
    let model_cfg: OnceLock<Result<aite_contracts::ModelConfig, String>> = OnceLock::new();
    Arc::new(move || {
        let cfg = model_cfg
            .get_or_init(|| {
                load_config(&config_path)
                    .map(|c| c.model)
                    .map_err(|e| e.to_string())
            })
            .clone()?;
        let env = aite_models::env_snapshot();
        OpenAiCompatModel::from_config(&cfg, &env)
            .map(|m| Arc::new(m) as Arc<dyn ModelPort>)
            .map_err(|e| format!("{e}（配置：{config_path}）"))
    })
}

/// `--sandbox docker`：edge 的真沙箱 + 真 `P0ToolGateway`，各套一层 R7 的探针。
///
/// 平台仍然是 `FakePlatform`（附件从它来、回执也发回它）—— 这一档验的是沙箱与 Gateway
/// 这一段真的走通了。`resolver` 是 R7 递过来的现成货：`P0ToolGateway` 的令牌校验是
/// 失败关闭的，不接就是每个工具调用都 `denied`，而报出来的是「denied」不是「你没接」。
///
/// CC4 ⑦：这一档**不接**功能接缝（登记与 `gateway_options`）—— 要接就得在同一场景里第二次跑
/// `wire_all`（`plane_factory` 已经跑过一次）。记账转 T0c（`wiring.rs` 的下一任 R0）。
fn docker_sandbox_factory(edge: Arc<LazyEdge>) -> SandboxFactory {
    Arc::new(move |platform, _store, config, resolver| {
        let edge = edge.connected()?;
        let probe = docker_sandbox_stack(edge.sandbox());
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

/// docker 档的沙箱那一摞，自下而上：
///
/// ```text
/// EdgeSandbox（gRPC → edge）
///   └─ LeasedSandbox   记账 + 收尾，**不进 CallLog**
///        └─ SandboxProbe  断言面（每一发都记进 CallLog）
/// ```
///
/// 抽成一个函数是为了让 `mod tests` 钉住的就是接线真正用的那一摞 —— 测试自己另搭一份的话，
/// 「记账层必须在探针下面」这条被人调换了顺序也照样绿。层序的理由见 [`LeasedSandbox`]。
fn docker_sandbox_stack(edge_sandbox: Arc<dyn SandboxPort>) -> Arc<SandboxProbe> {
    let leased: Arc<dyn SandboxPort> = Arc::new(LeasedSandbox::new(edge_sandbox));
    Arc::new(SandboxProbe::new(leased))
}

/// 租约记账：`acquire` 记下 sandbox_id，`release` / `reap_idle` 划掉，
/// `close_all` 把还留着的逐个还回去。其余方法原样转发。
///
/// **为什么需要它**：`EdgeSandbox::close_all` 是本地 no-op（容器记账在 edge 那边，
/// 见 `edge-client/src/sandbox.rs` 那句注释），于是 `Deps::close()`——runner 每个场景收尾
/// 都调的那一发——在 docker 档下**什么都没做**。任务正常收尾时容器由 worker 的
/// `release_task` 还掉；没走到终态的那些（场景超时被取消、断言前就炸了、plane 硬取消）
/// 就一直占着 `cpu` / `mem_mb`，活到 `reap_idle` 的 idle_sec（默认 300s）或者 `aite-edge`
/// 停机。十个场景连跑会叠加。
///
/// **必须裹在探针下面**（`edge.sandbox()` 与 `SandboxProbe::new` 之间）。裹在上面的话，
/// 它收尾时发的那几发 release 会经过 `SandboxProbe` 记进 `CallLog`，而 `Deps::close()`
/// 跑在 `run_checks()` **之前** —— `evals/p0/07_commands.yaml` 的
/// `{check: sandbox_calls, method: release, min: 1}` 和 JSON 摘要里的 `sandbox_calls`
/// 读到的就是被自己污染过的计数（`min` 侥幸挡得住，换成 `equals` 当场破）。裹在下面，
/// release 直接打给 `EdgeSandbox`，一笔都不进账。
///
/// **收尾失败不许盖掉场景结论**：release 失败只 `tracing::warn!`，`close_all` 永远回 `Ok`。
/// 一个「收尾没收干净」的警告不该把一个本来通过的场景判成失败。
///
/// 不做精细的「谁还活着」记账是安全的 —— edge 侧 `Release` 是幂等的
/// （`edge/internal/sandbox/docker.go`：不认识的 id、已经没了的容器，都当成已经释放），
/// 所以重复还一个已经被 worker 正常还掉的 id 不会出事。
struct LeasedSandbox {
    inner: Arc<dyn SandboxPort>,
    live: Mutex<BTreeSet<String>>,
}

impl LeasedSandbox {
    fn new(inner: Arc<dyn SandboxPort>) -> Self {
        Self {
            inner,
            live: Mutex::new(BTreeSet::new()),
        }
    }

    /// 中毒的锁照常用：记账面出岔子不该把评测带走（纪律 4：不 panic）。
    fn live(&self) -> std::sync::MutexGuard<'_, BTreeSet<String>> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait]
impl SandboxPort for LeasedSandbox {
    async fn acquire(&self, task_id: &str, spec: &SandboxSpec) -> Result<String, SandboxError> {
        let sandbox_id = self.inner.acquire(task_id, spec).await?;
        self.live().insert(sandbox_id.clone());
        Ok(sandbox_id)
    }

    async fn exec(&self, sandbox_id: &str, req: &ExecRequest) -> Result<ExecResult, SandboxError> {
        self.inner.exec(sandbox_id, req).await
    }

    async fn put_file(
        &self,
        sandbox_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), SandboxError> {
        self.inner.put_file(sandbox_id, path, data).await
    }

    async fn get_file(&self, sandbox_id: &str, path: &str) -> Result<Vec<u8>, SandboxError> {
        self.inner.get_file(sandbox_id, path).await
    }

    async fn list_files(&self, sandbox_id: &str) -> Result<Vec<String>, SandboxError> {
        self.inner.list_files(sandbox_id).await
    }

    async fn touch(&self, sandbox_id: &str) -> Result<(), SandboxError> {
        self.inner.touch(sandbox_id).await
    }

    async fn release(&self, sandbox_id: &str) -> Result<(), SandboxError> {
        // 先划账再发：发完再划的话，中间那一发要是抛了，这个 id 就永远划不掉，
        // `close_all` 会对一个早就没了的容器重复 release（幂等，不出事，但白吵一句 warn）。
        self.live().remove(sandbox_id);
        self.inner.release(sandbox_id).await
    }

    async fn reap_idle(&self, idle_sec: u32) -> Result<Vec<String>, SandboxError> {
        let released = self.inner.reap_idle(idle_sec).await?;
        let mut live = self.live();
        for id in &released {
            live.remove(id);
        }
        Ok(released)
    }

    /// 把这个场景还欠着的容器还回去。**永远回 `Ok`**（见类型文档最后一段）。
    async fn close_all(&self) -> Result<(), SandboxError> {
        let leftover: Vec<String> = std::mem::take(&mut *self.live()).into_iter().collect();
        for id in leftover {
            if let Err(e) = self.inner.release(&id).await {
                tracing::warn!(target: "aite.evals", sandbox = %id, error = %e, "evals.leftover_release_failed");
            }
        }
        // 转发给下一层：`EdgeSandbox` 那边是 no-op，但别在这里替它决定。
        if let Err(e) = self.inner.close_all().await {
            tracing::warn!(target: "aite.evals", error = %e, "evals.sandbox_close_all_failed");
        }
        Ok(())
    }
}

/// 起飞前体检：edge 通不通、契约版本对不对、daemon 在不在、镜像起不起得来。
///
/// 最后一条是真起一个容器（`acquire` 自带 `_check_ready`，不过就地 release），
/// 所以它同时覆盖了旧 `preflight.py` 第 6 项的「镜像存在」与「四个 import 能跑」。
/// **一定要 release**：否则体检自己留一个容器，验收那条 `docker ps -a --filter
/// label=aite.task` 就不是 0 了。
fn docker_probe(holder: Arc<LazyEdge>) -> DockerProbe {
    Arc::new(move |images: &[String]| {
        // **edge 连接就发生在这里**：体检这一步正是 Python `__main__.py` 里 docker 体检的
        // 位置，排在 parse / load_suite / `--only` / `--list` 之后。连不上就是体检没过，
        // 一行人话 + 退出码 2 —— 而不是把 `--list` 和 `-h` 一起挡在门外。
        let edge = match holder.connect() {
            Ok(edge) => edge,
            Err(e) => return Some(e),
        };
        let rt = match edge_runtime() {
            Ok(rt) => rt,
            Err(e) => return Some(e),
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use aite_testing::FakeSandbox;

    /// 场景那条 runtime 就是 current_thread（`evals::cli` 里那条），照它来。
    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("测试用 runtime")
            .block_on(fut)
    }

    fn spec() -> SandboxSpec {
        SandboxSpec::new("aite-sandbox:p0")
    }

    /// 场景没走到终态时留下的容器，要在 `Deps::close()` 那一发里被还掉。
    ///
    /// 病着的样子：`EdgeSandbox::close_all` 是本地 no-op，所以这一整发什么都不做，
    /// 容器一直占着 cpu/mem 活到 `reap_idle` 的 idle_sec 或者 edge 停机。
    /// `FakeSandbox` 不覆盖 `close_all`，用的是契约 trait 那个 `Ok(())` 默认实现 ——
    /// 跟 `EdgeSandbox` 一样的 no-op，所以这条用例复现的就是真实现那个病。
    #[test]
    fn leased_sandbox_releases_what_the_scenario_left_behind() {
        let inner = Arc::new(FakeSandbox::default());
        let leased = LeasedSandbox::new(inner.clone());
        block_on(async {
            let a = leased.acquire("task-a", &spec()).await.expect("acquire a");
            let b = leased.acquire("task-b", &spec()).await.expect("acquire b");
            leased.release(&a).await.expect("release a");
            assert_eq!(inner.alive(), vec![b.clone()], "a 该已经还掉了");

            leased.close_all().await.expect("close_all 永远回 Ok");
            assert!(
                inner.alive().is_empty(),
                "收尾没把 b 还回去：{:?}",
                inner.alive()
            );
            // 顺序 = a 是场景自己还的，b 是收尾补的；a 不该被补第二发。
            assert_eq!(inner.released_ids(), vec![a, b]);
        });
    }

    /// 收尾失败**不许**盖掉场景结论：release 抛了也只是少收一个容器，
    /// `close_all` 仍然回 `Ok`（真正的结论由 `run_checks` 判）。
    #[test]
    fn a_failing_cleanup_release_never_becomes_an_error() {
        struct RefusesToRelease;
        #[async_trait]
        impl SandboxPort for RefusesToRelease {
            async fn acquire(&self, _t: &str, _s: &SandboxSpec) -> Result<String, SandboxError> {
                Ok("sb-1".to_string())
            }
            async fn exec(&self, _id: &str, _r: &ExecRequest) -> Result<ExecResult, SandboxError> {
                unimplemented!("这条用例用不到")
            }
            async fn put_file(&self, _i: &str, _p: &str, _d: &[u8]) -> Result<(), SandboxError> {
                unimplemented!("这条用例用不到")
            }
            async fn get_file(&self, _i: &str, _p: &str) -> Result<Vec<u8>, SandboxError> {
                unimplemented!("这条用例用不到")
            }
            async fn list_files(&self, _i: &str) -> Result<Vec<String>, SandboxError> {
                unimplemented!("这条用例用不到")
            }
            async fn touch(&self, _i: &str) -> Result<(), SandboxError> {
                unimplemented!("这条用例用不到")
            }
            async fn release(&self, _id: &str) -> Result<(), SandboxError> {
                Err(SandboxError::new(
                    aite_contracts::SandboxErrorKind::Unavailable,
                    "edge 挂了",
                ))
            }
            async fn reap_idle(&self, _s: u32) -> Result<Vec<String>, SandboxError> {
                unimplemented!("这条用例用不到")
            }
        }

        let leased = LeasedSandbox::new(Arc::new(RefusesToRelease));
        block_on(async {
            leased.acquire("task-a", &spec()).await.expect("acquire");
            assert!(
                leased.close_all().await.is_ok(),
                "收尾失败不该变成 Err —— 那会经 SandboxProbe 记成 error，把本来通过的场景搅浑"
            );
        });
    }

    /// **层序**：收尾发出去的那几发 release 不许进断言面。
    ///
    /// `Deps::close()` 跑在 `run_checks()` **之前**，所以记账层要是裹在探针**外面**，
    /// `evals/p0/07_commands.yaml` 的 `{check: sandbox_calls, method: release}` 和 JSON
    /// 摘要里的 `sandbox_calls` 读到的就是被自己污染过的计数。
    /// 把 `docker_sandbox_stack` 里两层调个个儿，这条会红（release 记成 2 发）。
    #[test]
    fn cleanup_releases_stay_out_of_the_assertion_surface() {
        let inner = Arc::new(FakeSandbox::default());
        let probe = docker_sandbox_stack(inner.clone());
        block_on(async {
            let a = probe.acquire("task-a", &spec()).await.expect("acquire a");
            probe.acquire("task-b", &spec()).await.expect("acquire b");
            probe.release(&a).await.expect("release a");

            // runner 每个场景收尾都走这一发（`Deps::close`）。
            SandboxFacade::close(&*probe).await;

            assert!(
                inner.alive().is_empty(),
                "收尾没收干净：{:?}",
                inner.alive()
            );
            assert_eq!(
                probe.calls.count("release"),
                1,
                "只有场景自己那一发 release 该进 CallLog；收尾补的那些不算场景的行为"
            );
            assert_eq!(probe.calls.count("acquire"), 2);
        });
    }
}
