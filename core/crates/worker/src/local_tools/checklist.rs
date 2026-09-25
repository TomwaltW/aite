//! checklist_add / check / fail / note（CC3 从 `agent.rs` 原样搬来）。
use aite_contracts::{ChecklistItem, ChecklistState, EvidenceKind, ToolCallRequest, ToolErrorCode};
use serde_json::{Map, Value, json};

use crate::card::{MAX_ITEM_CHARS, clip};
use crate::r#loop::{AgentWorker, RunContext, ToolOutcome};
use crate::{RunError, fingerprint, texts};

pub const ENABLED: bool = true;

impl AgentWorker {
    pub(crate) async fn run_checklist(
        &self,
        ctx: &mut RunContext,
        call: &ToolCallRequest,
    ) -> Result<ToolOutcome, RunError> {
        let args = &call.arguments;
        match call.name.as_str() {
            "checklist_add" => {
                // 先整体校验再截到 8 项：第 9 项是空串照样算不合法（与 Python 同口径）
                let valid = args.get("items").and_then(Value::as_array).filter(|xs| {
                    !xs.is_empty()
                        && xs
                            .iter()
                            .all(|x| x.as_str().is_some_and(|s| !s.trim().is_empty()))
                });
                let Some(items) = valid else {
                    return self
                        .local_result(
                            ctx,
                            call,
                            false,
                            texts::CHECKLIST_ITEMS_INVALID,
                            Some(ToolErrorCode::InvalidArgs),
                        )
                        .await;
                };
                let mut added: Vec<String> = Vec::new();
                for text in items.iter().take(8) {
                    let id = format!("c{}", ctx.task.checklist.len() + 1);
                    ctx.task.checklist.push(ChecklistItem {
                        id: id.clone(),
                        text: clip(text.as_str().unwrap_or_default(), MAX_ITEM_CHARS),
                        state: ChecklistState::Todo,
                        note: None,
                    });
                    added.push(id);
                }
                let all_texts: Vec<String> =
                    ctx.task.checklist.iter().map(|i| i.text.clone()).collect();
                self.checklist_evidence(
                    &ctx.task.id,
                    "add",
                    json!({"ids": added, "items": all_texts}),
                )
                .await?;
                let content = texts::checklist_added(added.len(), &added);
                self.local_result(ctx, call, true, &content, None).await
            }
            "checklist_check" | "checklist_fail" => {
                let wanted = args.get("id").and_then(Value::as_str);
                let idx = wanted.and_then(|id| ctx.task.checklist.iter().position(|i| i.id == id));
                let Some(idx) = idx else {
                    let content =
                        texts::checklist_no_such_item(&fingerprint::py_repr(args.get("id")));
                    return self
                        .local_result(ctx, call, false, &content, Some(ToolErrorCode::InvalidArgs))
                        .await;
                };
                if call.name == "checklist_check" {
                    ctx.task.checklist[idx].state = ChecklistState::Done;
                } else {
                    let reason = args.get("reason").and_then(Value::as_str);
                    let Some(reason) = reason.filter(|r| !r.trim().is_empty()) else {
                        return self
                            .local_result(
                                ctx,
                                call,
                                false,
                                texts::CHECKLIST_REASON_REQUIRED,
                                Some(ToolErrorCode::InvalidArgs),
                            )
                            .await;
                    };
                    ctx.task.checklist[idx].state = ChecklistState::Failed;
                    ctx.task.checklist[idx].note = Some(clip(reason, 40));
                }
                let item_id = ctx.task.checklist[idx].id.clone();
                let state = ctx.task.checklist[idx].state;
                // BB2 ③：带上这一项的文本。原来只有 `id` + `state`，单看一条
                // `checklist_op` 读不懂 —— `aite evidence show` 靠回放前面的 `add`
                // 事件补了回来，但那意味着**每个消费方都得自己回放**。
                // 代价比 §8 估的小得多：`checklist_add` 那边已经 `clip(·, 20)` 过，
                // 一份副本最多 20 个字符，不是「文本不短」。
                let item_text = ctx.task.checklist[idx].text.clone();
                // "checklist_check"[10:] == "check"；"checklist_fail"[10:] == "fail"
                let op = &call.name[10..];
                self.checklist_evidence(
                    &ctx.task.id,
                    op,
                    json!({"id": item_id, "state": state, "text": item_text}),
                )
                .await?;
                let content = texts::checklist_marked(&item_id, state.as_str());
                self.local_result(ctx, call, true, &content, None).await
            }
            "checklist_note" => {
                let text = args.get("text").and_then(Value::as_str);
                let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
                    return self
                        .local_result(
                            ctx,
                            call,
                            false,
                            texts::CHECKLIST_TEXT_REQUIRED,
                            Some(ToolErrorCode::InvalidArgs),
                        )
                        .await;
                };
                // 备注只活在这一趟 run 里（卡片 footer 上），不落库
                ctx.note = Some(clip(text, 40));
                let note = ctx.note.clone().unwrap_or_default();
                self.checklist_evidence(&ctx.task.id, "note", json!({"text": note}))
                    .await?;
                self.local_result(ctx, call, true, texts::CHECKLIST_NOTE_UPDATED, None)
                    .await
            }
            other => {
                let content = texts::unknown_tool(other);
                self.local_result(ctx, call, false, &content, Some(ToolErrorCode::NotFound))
                    .await
            }
        }
    }

    pub(crate) async fn checklist_evidence(
        &self,
        task_id: &str,
        op: &str,
        extra: Value,
    ) -> Result<(), RunError> {
        let mut payload = Map::new();
        payload.insert("op".into(), Value::String(op.to_string()));
        if let Value::Object(rest) = extra {
            for (k, v) in rest {
                payload.insert(k, v);
            }
        }
        self.evidence
            .append(task_id, EvidenceKind::ChecklistOp, payload)
            .await?;
        Ok(())
    }
}
