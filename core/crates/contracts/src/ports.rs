//! 跨 crate / 跨轨的调用面（对应旧 aite/contracts/ports.py）。
//!
//! 实现者：PlatformPort = aite-edge-client（gRPC 到 Go edge）与 aite-testing（Fake）；
//! SandboxPort = aite-edge-client 与 aite-testing；SessionStore = aite-store；EvidenceWriter = aite-evidence；
//! ToolGateway = aite-gateway；ModelPort = aite-models；TaskWorker = aite-worker；ControlPlane = aite-control。
//! 各轨并行期间需要的替身在自己 crate 的 tests/ 里私写，不要跨轨依赖（重复远比冲突便宜）。
//!
//! 与旧版的差异（全部是把当年靠 getattr 探测的"协议外方法"显式化）：
//! - ToolGateway 多了 register_task / unregister_task / sandbox_id_of / release_task（沙箱归属）
//! - SessionStore 多了 close / recover_orphan_tasks / next_turn_seq
//! - EvidenceWriter 多了 task_dir
//! - SandboxPort 多了 close_all（旧 aclose）
//! - 新增 TaskWorker（旧 AgentWorker.run 的形状）与 RunHooks（drain_steer / is_cancelled 两个闭包）
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::capabilities::PlatformCapabilities;
use crate::errors::{
    EvidenceError, IngressError, ModelError, PlatformError, SandboxError, StoreError,
};
use crate::events::NormalizedEvent;
use crate::evidence::{EvidenceEvent, EvidenceKind};
use crate::gateway::{ToolContext, ToolResult};
use crate::outbound::{
    ChecklistCard, DocumentContent, HistoryMessage, OutboundFile, OutboundText, ReactionKind,
    SendResult,
};
use crate::protocol::{Message, ModelTurn, ToolCallRequest, ToolSpec};
use crate::sandbox::{ExecRequest, ExecResult, SandboxSpec};
use crate::session::{Session, Task, Turn};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 平台事件回调。必须在 1s 内返回（只做去重/入库/入队），重活不在回调里做。
pub type EventHandler =
    Arc<dyn Fn(NormalizedEvent) -> BoxFuture<'static, Result<(), IngressError>> + Send + Sync>;

#[async_trait]
pub trait PlatformPort: Send + Sync {
    fn capabilities(&self) -> PlatformCapabilities;
    /// 建立事件投递并持续投递事件；返回即表示投递面已就绪（不阻塞到 stop）。
    /// 断线自动重连；重连后平台重推的重复事件由 ControlPlane 靠 event_id 去重，adapter 不负责去重。
    async fn start(&self, on_event: EventHandler) -> Result<(), PlatformError>;
    async fn stop(&self) -> Result<(), PlatformError>;
    async fn send_text(&self, msg: &OutboundText) -> Result<SendResult, PlatformError>;
    async fn send_card(
        &self,
        chat_id: &str,
        reply_to: Option<&str>,
        card: &ChecklistCard,
    ) -> Result<SendResult, PlatformError>;
    /// 必须原地更新同一条消息，绝不新发消息
    async fn update_card(&self, card_id: &str, card: &ChecklistCard) -> Result<(), PlatformError>;
    async fn send_file(&self, msg: &OutboundFile) -> Result<SendResult, PlatformError>;
    async fn add_reaction(&self, message_id: &str, kind: ReactionKind)
    -> Result<(), PlatformError>;
    /// 按时间正序返回；不做 sender_kind 过滤（过滤归 Gateway 的 read_group_history 工具）
    async fn read_history(
        &self,
        chat_id: &str,
        limit: u32,
        thread_id: Option<&str>,
    ) -> Result<Vec<HistoryMessage>, PlatformError>;
    async fn read_document(&self, url_or_token: &str) -> Result<DocumentContent, PlatformError>;
    async fn download_file(
        &self,
        message_id: &str,
        file_key: &str,
    ) -> Result<Vec<u8>, PlatformError>;
}

#[async_trait]
pub trait ModelPort: Send + Sync {
    fn name(&self) -> String;
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        max_tokens: u32,
        temperature: f32,
    ) -> Result<ModelTurn, ModelError>;
}

#[async_trait]
pub trait SandboxPort: Send + Sync {
    /// 返回 sandbox_id；容器打标签 aite.task=<task_id>
    async fn acquire(&self, task_id: &str, spec: &SandboxSpec) -> Result<String, SandboxError>;
    async fn exec(&self, sandbox_id: &str, req: &ExecRequest) -> Result<ExecResult, SandboxError>;
    /// path 必须在 /work 下
    async fn put_file(&self, sandbox_id: &str, path: &str, data: &[u8])
    -> Result<(), SandboxError>;
    async fn get_file(&self, sandbox_id: &str, path: &str) -> Result<Vec<u8>, SandboxError>;
    async fn list_files(&self, sandbox_id: &str) -> Result<Vec<String>, SandboxError>;
    /// 刷新最近活动时间；未知 id 静默 no-op
    async fn touch(&self, sandbox_id: &str) -> Result<(), SandboxError>;
    /// 幂等；未知 id 静默 no-op
    async fn release(&self, sandbox_id: &str) -> Result<(), SandboxError>;
    /// 释放空闲超时的（含别的进程留下的孤儿），返回被释放的 id
    async fn reap_idle(&self, idle_sec: u32) -> Result<Vec<String>, SandboxError>;
    /// 进程收尾：释放本实例记账的全部沙箱（旧 aclose）。默认什么都不做。
    async fn close_all(&self) -> Result<(), SandboxError> {
        Ok(())
    }
}

#[async_trait]
pub trait ToolGateway: Send + Sync {
    /// P0 = gateway_tools() 原样
    fn catalog(&self, ctx: &ToolContext) -> Vec<ToolSpec>;
    /// 顺序：校验 session_token → 查工具存在 → 按 ToolSpec.parameters 校验 arguments → 执行（带超时）→ 截断 content → 返回
    /// 永远不失败：所有失败以 ToolResult{ok:false, error} 表达
    async fn call(&self, ctx: &ToolContext, req: &ToolCallRequest) -> ToolResult;
    /// 任务开跑前登记 session_token（令牌校验是失败关闭的：不登记则每个调用都 denied）
    fn register_task(&self, task_id: &str, session_token: &str);
    fn unregister_task(&self, task_id: &str);
    /// run_python 建的沙箱归 Gateway 记账；worker 取产物前先问这里
    async fn sandbox_id_of(&self, task_id: &str) -> Option<String>;
    /// 幂等：撤 token + 释放该任务的沙箱
    async fn release_task(&self, task_id: &str);
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    /// 建表，幂等
    async fn init(&self) -> Result<(), StoreError>;
    async fn close(&self) -> Result<(), StoreError>;
    async fn get_session(&self, session_id: &str) -> Result<Option<Session>, StoreError>;
    /// 只找非 archived 的；多条时取 created_at 最新
    async fn find_session_by_thread(
        &self,
        chat_id: &str,
        thread_id: &str,
    ) -> Result<Option<Session>, StoreError>;
    async fn create_session(&self, s: &Session) -> Result<(), StoreError>;
    async fn update_session(&self, s: &Session) -> Result<(), StoreError>;
    /// seq 由调用方分配，重复 (session_id, seq) → StoreError::DuplicateTurn
    async fn append_turn(&self, t: &Turn) -> Result<(), StoreError>;
    /// 最近 limit 轮，按 seq 正序返回
    async fn list_turns(&self, session_id: &str, limit: u32) -> Result<Vec<Turn>, StoreError>;
    /// 下一条 turn 该用的 seq（= 现有最大 seq + 1，空则 0）
    async fn next_turn_seq(&self, session_id: &str) -> Result<u64, StoreError>;
    async fn create_task(&self, t: &Task) -> Result<(), StoreError>;
    async fn update_task(&self, t: &Task) -> Result<(), StoreError>;
    async fn get_task(&self, task_id: &str) -> Result<Option<Task>, StoreError>;
    /// status in ACTIVE_TASK_STATUSES，按 (created_at, id) 正序
    async fn list_active_tasks(&self, chat_id: &str) -> Result<Vec<Task>, StoreError>;
    /// 原子递增 + encode_task_no
    async fn next_task_no(&self, tenant_id: &str) -> Result<String, StoreError>;
    /// 去重：首次调用记录并返回 false，之后 true。写失败必须返回 Err，不许假装 false。
    async fn seen_event(&self, event_id: &str) -> Result<bool, StoreError>;
    /// 起飞时收上一条命的残局：所有活跃态任务改 failed + result_summary，返回被收拾的任务。
    /// 故意不在 init() 里自动调用（回帖与 evidence 归调用方）。
    async fn recover_orphan_tasks(&self) -> Result<Vec<Task>, StoreError>;
}

#[async_trait]
pub trait EvidenceWriter: Send + Sync {
    async fn append(
        &self,
        task_id: &str,
        kind: EvidenceKind,
        payload: Map<String, Value>,
    ) -> Result<EvidenceEvent, EvidenceError>;
    /// 写 manifest，返回 root_hash（空链返回 GENESIS）
    async fn finalize(
        &self,
        task_id: &str,
        manifest_extra: Map<String, Value>,
    ) -> Result<String, EvidenceError>;
    /// 重算整条链；只回答"文件此刻可不可信"，不自愈
    fn verify(&self, task_id: &str) -> bool;
    /// {evidence_dir}/{task_id}
    fn task_dir(&self, task_id: &str) -> PathBuf;
}

/// 控制面注入给 worker 的两个同步回调。
#[derive(Clone)]
pub struct RunHooks {
    /// 取走并清空该任务排队中的追问（steer），每步开始前调用
    pub drain_steer: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    /// 任务是否已被 !stop / 卡片停止
    pub is_cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl RunHooks {
    pub fn none() -> Self {
        Self {
            drain_steer: Arc::new(Vec::new),
            is_cancelled: Arc::new(|| false),
        }
    }
}

impl std::fmt::Debug for RunHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RunHooks")
    }
}

/// Agent loop（旧 AgentWorker.run）。返回跑完后的 Task（终态之一）；内部所有异常都要收敛成 failed，不外抛。
#[async_trait]
pub trait TaskWorker: Send + Sync {
    async fn run(
        &self,
        task: Task,
        session: Session,
        initiator: Option<String>,
        hooks: RunHooks,
    ) -> Task;
    /// 此刻在飞的任务（app 收尾时给硬取消的任务善终用）
    fn in_flight(&self) -> Vec<(Task, Session)>;
}

#[async_trait]
pub trait ControlPlane: Send + Sync {
    /// §3.5 路由规则的唯一入口
    async fn handle_event(&self, ev: NormalizedEvent) -> Result<(), IngressError>;
    /// 派发队列里的任务给 worker、跑沙箱 reaper；直到被取消
    async fn run_forever(&self);
    /// 只把当前排队的跑完（评测/测试用），不起 reaper
    async fn run_pending(&self);
    /// 排队中的任务数
    fn pending(&self) -> usize;
    /// 等队列清空
    async fn join(&self);
    /// !stop / 卡片停止 / 收尾硬取消 共用的取消路径
    async fn cancel_task(
        &self,
        task: Task,
        reply_to: Option<String>,
        chat_id: Option<String>,
        notify: bool,
    ) -> Task;
    /// 计数器快照（events.duplicate / events.nonhuman / ...）
    fn counters(&self) -> Map<String, Value>;
}
