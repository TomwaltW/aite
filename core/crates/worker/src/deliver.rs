//! 收尾（W5）：交付 / 失败 / 取消 / 定稿 / 落库 / 取产物。CC3 从 `agent.rs` 原样搬来。
use std::path::Path;

use aite_contracts::{
    CardStatus, EvidenceKind, OutboundFile, OutboundText, SandboxSpec, Task, TaskStatus,
};
use chrono::Utc;
use serde_json::{Map, Value, json};

use crate::card::clip;
use crate::local_tools::{is_truthy, plain_string};
use crate::r#loop::{AgentWorker, RunContext, json_object, sha256_hex};
use crate::{RunError, mime, texts};

impl AgentWorker {
    // ---- 收尾 ----------------------------------------------------------

    /// W5：产物逐个 get_file → send_file → evidence artifact；然后 send_text；delivered；finalize。
    pub(crate) async fn deliver(
        &self,
        ctx: &mut RunContext,
        reply: &str,
        artifacts: &[Map<String, Value>],
        answering: bool,
    ) -> Result<Task, RunError> {
        // Answering 路径（W3：第一步就 final，没发过卡片）在状态机上是独立的一格
        ctx.task.status = if answering {
            TaskStatus::Answering
        } else {
            TaskStatus::Working
        };
        self.save(ctx).await?;

        let thread_root = ctx.thread_root();
        let mut missing: Vec<String> = Vec::new();
        for art in artifacts {
            let path = plain_string(art.get("path"));
            // Python 是 `str(art.get("title") or art.get("path", ""))`（loop.py:464）：
            // 退回 `path` 的判据是**真值语义**，不是「字符串化之后为空」。
            // 弱模型给 `title: false` / `0` / `{}` 时，后者会把标题发成字面量 "false"。
            let title = if art.get("title").is_some_and(is_truthy) {
                plain_string(art.get("title"))
            } else {
                path.clone()
            };
            let Some(data) = self.fetch_artifact(ctx, &path).await else {
                missing.push(if title.is_empty() {
                    path.clone()
                } else {
                    title
                });
                continue;
            };
            // 同一条真值语义：Python 是
            // `art.get("mime") or mimetypes.guess_type(path)[0] or "application/octet-stream"`
            // （loop.py:469）。
            let mime = if art.get("mime").is_some_and(is_truthy) {
                plain_string(art.get("mime"))
            } else {
                mime::guess(&path)
                    .unwrap_or("application/octet-stream")
                    .to_string()
            };
            let name = Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| title.clone());
            self.platform
                .send_file(&OutboundFile {
                    chat_id: ctx.session.chat_id.clone(),
                    reply_to: Some(thread_root.clone()),
                    name,
                    mime: mime.clone(),
                    data: data.clone(),
                })
                .await?;
            self.append_evidence(
                &ctx.task.id,
                EvidenceKind::Artifact,
                json!({
                    "title": title,
                    "mime": mime,
                    "sha256": sha256_hex(&data),
                    "size": data.len(),
                }),
            )
            .await?;
        }

        let mut text = reply.trim().to_string();
        if !missing.is_empty() {
            let lines: Vec<String> = missing.iter().map(|m| texts::artifact_missing(m)).collect();
            text.push('\n');
            text.push_str(&lines.join("\n"));
        }
        self.platform
            .send_text(&OutboundText {
                chat_id: ctx.session.chat_id.clone(),
                text,
                reply_to: Some(thread_root),
                in_thread: true,
            })
            .await?;

        ctx.task.status = TaskStatus::Delivered;
        ctx.task.result_summary = clip(reply, 200);
        // 收卡片在写终态证据**之前**（BB2 ② 起）。卡片推送现在自己会写一条
        // `card_updated` 证据，而 `delivered` / `failed` / `cancelled` 必须是链上
        // 最后一条 —— `evidence show` 的「终态」就是拿最后一行的 kind 认的
        // （`cli.rs::is_terminal_kind`），`app/tests` 那两条贯通用例也钉着这一点。
        // 先收卡片、再落终态，链的收口顺序才和「任务真的结束了」对得上。
        self.close_card(ctx, CardStatus::Delivered).await?;
        self.append_evidence(
            &ctx.task.id,
            EvidenceKind::Delivered,
            json!({
                "artifacts": artifacts.len() - missing.len(),
                "missing": missing,
                "steps": ctx.task.steps,
            }),
        )
        .await?;
        self.finish(ctx).await?;
        Ok(ctx.task.clone())
    }

    pub(crate) async fn fail(&self, ctx: &mut RunContext, text: &str) -> Result<Task, RunError> {
        ctx.task.status = TaskStatus::Failed;
        ctx.task.result_summary = clip(text, 200);
        let msg = OutboundText {
            chat_id: ctx.session.chat_id.clone(),
            text: text.to_string(),
            reply_to: Some(ctx.thread_root()),
            in_thread: true,
        };
        // 发不出去也要把状态与证据落全：回帖只是通知，不是失败面的一部分
        if let Err(err) = self.platform.send_text(&msg).await {
            tracing::error!(task = %ctx.task.id, %err, "worker.fail_notice_failed");
        }
        self.close_card(ctx, CardStatus::Failed).await?; // 次序理由见 deliver()
        self.append_evidence(
            &ctx.task.id,
            EvidenceKind::Failed,
            json!({"reason": text, "steps": ctx.task.steps}),
        )
        .await?;
        self.finish(ctx).await?;
        Ok(ctx.task.clone())
    }

    pub(crate) async fn cancel(&self, ctx: &mut RunContext) -> Result<Task, RunError> {
        ctx.task.status = TaskStatus::Cancelled;
        self.close_card(ctx, CardStatus::Cancelled).await?; // 次序理由见 deliver()
        self.append_evidence(
            &ctx.task.id,
            EvidenceKind::Cancelled,
            json!({"steps": ctx.task.steps}),
        )
        .await?;
        self.finish(ctx).await?;
        Ok(ctx.task.clone())
    }

    pub(crate) async fn finish(&self, ctx: &mut RunContext) -> Result<(), RunError> {
        let model = if ctx.task.model.is_empty() {
            self.model.name()
        } else {
            ctx.task.model.clone()
        };
        let manifest = json_object(json!({
            "session_id": ctx.session.id,
            "task_no": ctx.task.task_no,
            "created_by": ctx.task.created_by,
            "model": model,
        }));
        ctx.task.evidence_root_hash = Some(self.evidence.finalize(&ctx.task.id, manifest).await?);
        // delivered / failed / cancelled 三条路都汇到这里，沙箱在这里还。
        // 不还的话容器要挂到 reaper 的 idle_sec 空闲超时才被收，任务结束了还占着。
        if let Some(gateway) = &self.gateway {
            gateway.release_task(&ctx.task.id).await;
        }
        self.save(ctx).await
    }

    pub(crate) async fn save(&self, ctx: &mut RunContext) -> Result<(), RunError> {
        ctx.task.updated_at = Utc::now();
        self.store.update_task(&ctx.task).await?;
        if let Ok(mut live) = self.in_flight.lock()
            && let Some(slot) = live.get_mut(&ctx.task.id)
        {
            slot.0 = ctx.task.clone();
        }
        Ok(())
    }

    /// §3.3：path 不在 /work 下或取不到 → 跳过该产物，任务仍 delivered。
    pub(crate) async fn fetch_artifact(&self, ctx: &mut RunContext, path: &str) -> Option<Vec<u8>> {
        if !path.starts_with("/work/") {
            return None;
        }
        let sandbox = self.sandbox.clone()?;
        if ctx.task.sandbox_id.is_none() {
            // 产物是 run_python 在 Gateway 那个沙箱里写出来的，得先问它要。
            // 不问就直接 acquire 的话拿到的是个全新的空容器，产物必然找不到。
            if let Some(gateway) = &self.gateway {
                ctx.task.sandbox_id = gateway.sandbox_id_of(&ctx.task.id).await;
            }
        }
        if ctx.task.sandbox_id.is_none() {
            let cfg = &self.config.sandbox;
            let spec = SandboxSpec {
                image: cfg.image.clone(),
                cpu: cfg.cpu,
                mem_mb: cfg.mem_mb,
                ..SandboxSpec::new(cfg.image.clone())
            };
            match sandbox.acquire(&ctx.task.id, &spec).await {
                Ok(id) => ctx.task.sandbox_id = Some(id),
                Err(err) => {
                    tracing::error!(task = %ctx.task.id, %path, %err, "worker.artifact_failed");
                    return None;
                }
            }
        }
        let sandbox_id = ctx.task.sandbox_id.clone()?;
        match sandbox.get_file(&sandbox_id, path).await {
            Ok(data) => Some(data),
            Err(err) => {
                tracing::error!(task = %ctx.task.id, %path, %err, "worker.artifact_failed");
                None
            }
        }
    }
}
