//! `EdgePlatform`：`PlatformPort` 的 gRPC 实现（core → edge 的 `PlatformService`）。
//!
//! 每个方法都是一条直路：domain → pb → RPC → pb → domain，`Status` 经
//! `aite_proto::status::platform_error_from_status`（R0 冻结的映射）翻成 `PlatformError`。
//!
//! 与旧 `PlatformPort` 的差异（spec §3.1 的 D1）：`start/stop` 不是「连飞书」而是
//! 「起/停 core 侧的 IngressService 监听」—— 长连接的生命周期归 edge 进程自己。
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use aite_contracts::{
    ChecklistCard, DocumentContent, EventHandler, HistoryMessage, OutboundFile, OutboundText,
    PlatformCapabilities, PlatformError, PlatformPort, ReactionKind, SendResult, feishu_p0,
};
use aite_proto::pb;
use async_trait::async_trait;

use crate::ingress::IngressServer;
use crate::link::Link;

pub struct EdgePlatform {
    link: Arc<Link>,
    ingress: IngressServer,
    /// 首次 `GetCapabilities` 的结果（`capabilities()` 是同步方法，拿不到就用 P0 默认值兜）
    capabilities: Mutex<Option<PlatformCapabilities>>,
}

impl EdgePlatform {
    pub(crate) fn new(link: Arc<Link>, core_socket: PathBuf) -> Self {
        let max_bytes = link.max_bytes();
        Self {
            link,
            ingress: IngressServer::new(core_socket, max_bytes),
            capabilities: Mutex::new(None),
        }
    }

    /// 起没起 IngressService（= 投递面在不在）。
    pub fn started(&self) -> bool {
        self.ingress.started()
    }

    pub fn ingress(&self) -> &IngressServer {
        &self.ingress
    }

    /// 问一次 edge 的 capabilities 并缓存；已经缓存过就直接给缓存（「缓存首次」）。
    pub async fn refresh_capabilities(&self) -> Result<PlatformCapabilities, PlatformError> {
        if let Some(cached) = self.capabilities.lock().expect("capabilities 锁").clone() {
            return Ok(cached);
        }
        let reply = self
            .link
            .platform_client()
            .get_capabilities(pb::GetCapabilitiesRequest {})
            .await
            .map_err(|st| self.link.platform_error(&st))
            .inspect(|_| self.link.note_ok())?;
        let caps = PlatformCapabilities::from(reply.into_inner());
        *self.capabilities.lock().expect("capabilities 锁") = Some(caps.clone());
        Ok(caps)
    }
}

#[async_trait]
impl PlatformPort for EdgePlatform {
    /// trait 这一面是同步的，但 capabilities 在 edge 那边 —— 所以是缓存值：
    /// `start()` 会顺手问一次；问到之前按 P0 冻结的飞书能力回答（RΩ 起飞时还会比对 contract_version）。
    fn capabilities(&self) -> PlatformCapabilities {
        self.capabilities
            .lock()
            .expect("capabilities 锁")
            .clone()
            .unwrap_or_else(feishu_p0)
    }

    async fn start(&self, on_event: EventHandler) -> Result<(), PlatformError> {
        self.ingress.start(on_event).await?;
        // edge 还没起来也不挡 core 起飞（§2.1 启动顺序无关）：能力表下次再问。
        if let Err(e) = self.refresh_capabilities().await {
            tracing::warn!(error = %e, "edge.capabilities_unavailable");
        }
        Ok(())
    }

    async fn stop(&self) -> Result<(), PlatformError> {
        self.ingress.stop().await
    }

    async fn send_text(&self, msg: &OutboundText) -> Result<SendResult, PlatformError> {
        let reply = self
            .link
            .platform_client()
            .send_text(pb::OutboundText::from(msg.clone()))
            .await
            .map_err(|st| self.link.platform_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(SendResult::from(reply.into_inner()))
    }

    async fn send_card(
        &self,
        chat_id: &str,
        reply_to: Option<&str>,
        card: &ChecklistCard,
    ) -> Result<SendResult, PlatformError> {
        let reply = self
            .link
            .platform_client()
            .send_card(pb::SendCardRequest {
                chat_id: chat_id.to_string(),
                reply_to: reply_to.map(str::to_string),
                card: Some(pb::ChecklistCard::from(card.clone())),
            })
            .await
            .map_err(|st| self.link.platform_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(SendResult::from(reply.into_inner()))
    }

    async fn update_card(&self, card_id: &str, card: &ChecklistCard) -> Result<(), PlatformError> {
        self.link
            .platform_client()
            .update_card(pb::UpdateCardRequest {
                card_id: card_id.to_string(),
                card: Some(pb::ChecklistCard::from(card.clone())),
            })
            .await
            .map_err(|st| self.link.platform_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(())
    }

    async fn send_file(&self, msg: &OutboundFile) -> Result<SendResult, PlatformError> {
        let reply = self
            .link
            .platform_client()
            .send_file(pb::OutboundFile::from(msg.clone()))
            .await
            .map_err(|st| self.link.platform_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(SendResult::from(reply.into_inner()))
    }

    async fn add_reaction(
        &self,
        message_id: &str,
        kind: ReactionKind,
    ) -> Result<(), PlatformError> {
        self.link
            .platform_client()
            .add_reaction(pb::AddReactionRequest {
                message_id: message_id.to_string(),
                kind: pb::ReactionKind::from(kind) as i32,
            })
            .await
            .map_err(|st| self.link.platform_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(())
    }

    async fn read_history(
        &self,
        chat_id: &str,
        limit: u32,
        thread_id: Option<&str>,
    ) -> Result<Vec<HistoryMessage>, PlatformError> {
        let reply = self
            .link
            .platform_client()
            .read_history(pb::ReadHistoryRequest {
                chat_id: chat_id.to_string(),
                limit: limit as i32,
                thread_id: thread_id.map(str::to_string),
            })
            .await
            .map_err(|st| self.link.platform_error(&st))
            .inspect(|_| self.link.note_ok())?;
        reply
            .into_inner()
            .messages
            .into_iter()
            .map(|m| HistoryMessage::try_from(m).map_err(Link::bad_response))
            .collect()
    }

    async fn read_document(&self, url_or_token: &str) -> Result<DocumentContent, PlatformError> {
        let reply = self
            .link
            .platform_client()
            .read_document(pb::ReadDocumentRequest {
                url_or_token: url_or_token.to_string(),
            })
            .await
            .map_err(|st| self.link.platform_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(DocumentContent::from(reply.into_inner()))
    }

    async fn download_file(
        &self,
        message_id: &str,
        file_key: &str,
    ) -> Result<Vec<u8>, PlatformError> {
        let reply = self
            .link
            .platform_client()
            .download_file(pb::DownloadFileRequest {
                message_id: message_id.to_string(),
                file_key: file_key.to_string(),
            })
            .await
            .map_err(|st| self.link.platform_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(reply.into_inner().data)
    }
}
