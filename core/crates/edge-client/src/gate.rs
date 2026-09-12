//! 契约版本闸门 —— 「连上就比一次」，比出不一致就闸断这条链路（§2.1）。
//!
//! **为什么起飞那一次不够。** `build_app` 的第 4 步比对 `contract_version`：拨得通就严格
//! 比，不等就拒绝起飞；最多 5 次 × 1s 都拨不通就记一行 `aite.edge_unreachable` 照常起飞
//! （§2.1「启动顺序无关，连不上不退出」）。问题不在这个折中本身，在**此后整个进程
//! 生命周期再也没有第二次比对**。三条常态路径都会落进这个缺口：
//!
//! 1. `docker-compose.yml` 刻意不写 `depends_on`（就是 §2.1 那条「谁先起都行」）。
//!    core 先起完、edge 编译/启动超过 ~4s 才就绪，是完全正常的一次 `docker compose up`。
//! 2. M6 三遍里的「只重启 edge」那一遍：core 长活不动、edge 换了新契约起来 ——
//!    这条路上门禁**必然**不生效，没有任何随机性。
//! 3. 真机排障时手动重启 aite-edge，同理。
//!
//! 于是「两个进程各拿一半契约在跑」这件事可以一声不响地持续下去，而 §2.1 明令它该拒绝。
//!
//! **这里怎么补。** 把比对下沉到 `EdgeClient` 自己：`link.rs` 的后台探针是「重新连上」的
//! 唯一事件源，它每拨通一次就发一发 `GetStatus` 比一次；`EdgeClient::status()`（起飞体检
//! 和健康行走的那条）每答上来一次也顺手记一次。比出不一致就落闸，此后每发 platform /
//! sandbox RPC 直接失败并带人话。
//!
//! **为什么是闸门而不是退出。** M6 的「只重启 edge」那一遍，core 自己也退的话，compose 的
//! `restart: unless-stopped` 会把两个进程拖进互相重启；只记一行 ERROR 又太轻（真机上没人
//! 盯日志，症状会变成「@ 了机器人没反应」）。闸门是「不干活但不自杀」：说得出为什么、
//! 人一把 edge 换回对的版本就自己恢复。
//!
//! **自愈怎么走。** 闸落下之后所有 RPC 都被拦在本地，不再产生 `UNAVAILABLE`，探针也就
//! 不会被 `note()` 叫醒 —— 所以取 client 被拒的那一下**顺手把探针叫起来**（`link.rs` 里
//! 那一句 `self.probe()`）。探针拨通一次就重比一次版本，对上了闸门自己开，不用重启 core。
use std::path::Path;
use std::sync::Mutex;

use aite_contracts::{CONTRACT_VERSION, PlatformError, SandboxError, SandboxErrorKind};

/// 闸门的三态。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ContractState {
    /// 还没比成过（edge 还没起、或者 `GetStatus` 没答上来）。
    ///
    /// **放行**。拦在这里的话 §2.1 的懒连接就废了：core 先起时第一发 RPC 本来就该拿一个
    /// retryable 的错误，而不是被自己人挡住。
    #[default]
    Unverified,
    /// 比过，一致。
    Ok,
    /// 比过，不一致。此后每发 platform / sandbox RPC 在本地直接失败。
    Mismatch {
        edge_contract: String,
        edge_version: String,
    },
}

impl ContractState {
    /// 闸是不是落着。
    pub fn is_barred(&self) -> bool {
        matches!(self, ContractState::Mismatch { .. })
    }
}

pub(crate) struct ContractGate {
    state: Mutex<ContractState>,
}

impl ContractGate {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(ContractState::Unverified),
        }
    }

    /// 比一次并记下结论。
    ///
    /// 一致就**解闸** —— 这就是自愈那一半：edge 换回对的版本、探针再连上一次，闸门自己开。
    /// 状态没变化时一个字都不打，免得探针每轮都刷屏。
    pub(crate) fn observe(&self, edge_contract: &str, edge_version: &str, socket: &Path) {
        let next = if edge_contract == CONTRACT_VERSION {
            ContractState::Ok
        } else {
            ContractState::Mismatch {
                edge_contract: edge_contract.to_string(),
                edge_version: edge_version.to_string(),
            }
        };
        let mut state = self.state.lock().expect("contract gate 锁");
        if *state == next {
            return;
        }
        match (&*state, &next) {
            (ContractState::Mismatch { .. }, ContractState::Ok) => tracing::info!(
                target: "aite.edge",
                socket = %socket.display(),
                contract_version = %edge_contract,
                edge_version = %edge_version,
                "edge.contract_recovered 两边契约版本又对上了，链路已解闸"
            ),
            (_, ContractState::Ok) => tracing::info!(
                target: "aite.edge",
                socket = %socket.display(),
                contract_version = %edge_contract,
                edge_version = %edge_version,
                "edge.contract_ok"
            ),
            (_, ContractState::Mismatch { .. }) => tracing::error!(
                target: "aite.edge",
                socket = %socket.display(),
                core_contract_version = %CONTRACT_VERSION,
                edge_contract_version = %edge_contract,
                edge_version = %edge_version,
                "edge.contract_mismatch 两边契约版本不一致，这条链路已闸断"
            ),
            _ => {}
        }
        *state = next;
    }

    pub(crate) fn state(&self) -> ContractState {
        self.state.lock().expect("contract gate 锁").clone()
    }

    /// 闸落着就给出那条人话（含怎么恢复）。
    fn barred(&self) -> Option<String> {
        match &*self.state.lock().expect("contract gate 锁") {
            ContractState::Mismatch {
                edge_contract,
                edge_version,
            } => Some(format!(
                "两边契约版本不一致，到 edge 的调用已闸断：core CONTRACT_VERSION={CONTRACT_VERSION}，\
                 edge contract_version={edge_contract}（edge 版本 {edge_version}）。\
                 两个进程要一起升 —— 重新 `cargo build` + `go build` 之后再起 aite-edge；\
                 edge 换回对得上的版本之后本进程会自己恢复，不用重启 core。"
            )),
            _ => None,
        }
    }

    /// 不是抖动，是两个进程版本不配 —— 人不介入不会变好，所以 `retryable: false`。
    pub(crate) fn platform_error(&self) -> Option<PlatformError> {
        self.barred()
            .map(|message| PlatformError::new("contract_mismatch", message, false))
    }

    pub(crate) fn sandbox_error(&self) -> Option<SandboxError> {
        self.barred()
            .map(|message| SandboxError::new(SandboxErrorKind::Internal, message))
    }
}
