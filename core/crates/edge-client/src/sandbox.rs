//! `EdgeSandbox`：`SandboxPort` 的 gRPC 实现（core → edge 的 `SandboxService`）。
//!
//! 与 `EdgePlatform` 同一套路，只是 `Status` 走 `sandbox_error_from_status`：
//! NOT_FOUND 按 message 前缀分 `sandbox_not_found:` / `file_not_found:` 两种 kind，
//! INVALID_ARGUMENT 是路径不合规，UNAVAILABLE 是 docker daemon 不可达（这些都是 R0 冻结的映射）。
use std::sync::Arc;

use aite_contracts::{ExecRequest, ExecResult, SandboxError, SandboxPort, SandboxSpec};
use aite_proto::pb;
use async_trait::async_trait;

use crate::link::Link;

pub struct EdgeSandbox {
    link: Arc<Link>,
}

impl EdgeSandbox {
    pub(crate) fn new(link: Arc<Link>) -> Self {
        Self { link }
    }
}

#[async_trait]
impl SandboxPort for EdgeSandbox {
    async fn acquire(&self, task_id: &str, spec: &SandboxSpec) -> Result<String, SandboxError> {
        let reply = self
            .link
            .sandbox_client()?
            .acquire(pb::AcquireRequest {
                task_id: task_id.to_string(),
                spec: Some(pb::SandboxSpec::from(spec.clone())),
            })
            .await
            .map_err(|st| self.link.sandbox_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(reply.into_inner().sandbox_id)
    }

    async fn exec(&self, sandbox_id: &str, req: &ExecRequest) -> Result<ExecResult, SandboxError> {
        let reply = self
            .link
            .sandbox_client()?
            .exec(pb::ExecCallRequest {
                sandbox_id: sandbox_id.to_string(),
                req: Some(pb::ExecRequest::from(req.clone())),
            })
            .await
            .map_err(|st| self.link.sandbox_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(ExecResult::from(reply.into_inner()))
    }

    async fn put_file(
        &self,
        sandbox_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), SandboxError> {
        self.link
            .sandbox_client()?
            .put_file(pb::PutFileRequest {
                sandbox_id: sandbox_id.to_string(),
                path: path.to_string(),
                data: data.to_vec(),
            })
            .await
            .map_err(|st| self.link.sandbox_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(())
    }

    async fn get_file(&self, sandbox_id: &str, path: &str) -> Result<Vec<u8>, SandboxError> {
        let reply = self
            .link
            .sandbox_client()?
            .get_file(pb::GetFileRequest {
                sandbox_id: sandbox_id.to_string(),
                path: path.to_string(),
            })
            .await
            .map_err(|st| self.link.sandbox_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(reply.into_inner().data)
    }

    async fn list_files(&self, sandbox_id: &str) -> Result<Vec<String>, SandboxError> {
        let reply = self
            .link
            .sandbox_client()?
            .list_files(pb::ListFilesRequest {
                sandbox_id: sandbox_id.to_string(),
            })
            .await
            .map_err(|st| self.link.sandbox_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(reply.into_inner().paths)
    }

    async fn touch(&self, sandbox_id: &str) -> Result<(), SandboxError> {
        self.link
            .sandbox_client()?
            .touch(pb::TouchRequest {
                sandbox_id: sandbox_id.to_string(),
            })
            .await
            .map_err(|st| self.link.sandbox_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(())
    }

    async fn release(&self, sandbox_id: &str) -> Result<(), SandboxError> {
        self.link
            .sandbox_client()?
            .release(pb::ReleaseRequest {
                sandbox_id: sandbox_id.to_string(),
            })
            .await
            .map_err(|st| self.link.sandbox_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(())
    }

    async fn reap_idle(&self, idle_sec: u32) -> Result<Vec<String>, SandboxError> {
        let reply = self
            .link
            .sandbox_client()?
            .reap_idle(pb::ReapIdleRequest {
                idle_sec: idle_sec as i32,
            })
            .await
            .map_err(|st| self.link.sandbox_error(&st))
            .inspect(|_| self.link.note_ok())?;
        Ok(reply.into_inner().released)
    }

    /// 本地 no-op：沙箱记账在 edge（进程收尾时 edge 自己收容器），core 这边没有要释放的东西。
    async fn close_all(&self) -> Result<(), SandboxError> {
        Ok(())
    }
}
