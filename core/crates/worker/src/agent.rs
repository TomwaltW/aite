//! Agent Loop（§3.6 W1–W9）。对应旧 `aite/worker/loop.py` + `aite/app.py::AppWorker`。
//!
//! 一步 = 一次 `ModelPort::chat`。模型只能通过工具驱动进度面与产出：
//! checklist_* 与 final 由 worker 本地处理，其余交 `ToolGateway::call`（W2）。
//!
//! 旧版 `AppWorker` 只多做两件事（登记 session_token、记 in_flight），Rust 版并进来 ——
//! 冻结的 `ToolGateway` 已经显式化了 `register_task / release_task`，没必要再套一层。
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use aite_contracts::{
    AiteConfig, CardStatus, ChecklistItem, ChecklistState, EvidenceKind, EvidenceWriter, Message,
    ModelError, ModelPort, ModelTurn, OutboundFile, OutboundText, PlatformPort, Role, RunHooks,
    SandboxPort, SandboxSpec, Session, SessionStore, Task, TaskStatus, ToolCallRequest,
    ToolContext, ToolErrorCode, ToolGateway, all_model_tools, final_tool, is_local_tool,
};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::card::{CardCoalescer, MAX_ITEM_CHARS, MAX_TITLE_CHARS, clip, render_card};
use crate::context::{build_context, load_system_prompt};
use crate::{Clock, RunError, Sleeper, fingerprint, mime, texts};

/// §3.3 模型调用异常 / 5xx：重试 2 次（首发 + 两次退避，共 3 次调用）
pub const MODEL_RETRY_DELAYS: [f64; 2] = [2.0, 5.0];
/// §3.3 同一任务连续 N 次 → failed
pub const MAX_CONSECUTIVE_INVALID_ARGS: u32 = 3;
pub const MAX_CONSECUTIVE_SANDBOX_ERRORS: u32 = 2;

// 同一张牌原样连出好几次 —— §3.3 那张失败面表里一条都接不住的那种卡死。
// T17 实测 04_csv_to_chart：替身沙箱对不含 savefig 的代码一律回 exit_code=0 + 空 stdout，
// 模型自己诊断了几步之后，对**逐字节相同**的 run_python 连发 33 次，一路烧到 max_steps
// 才停（evals/live-report-2026-09-10.md §4 缺口 2）。既有两条计数兜底都要求工具**失败**
// ——invalid_args 要参数不合 schema，sandbox 要工具报错——而这里工具返回的是 ok=True，
// 只是内容为空。唯一接住它的是 max_steps，代价是烧满整整 40 次模型调用。
pub const REPEAT_NUDGE_AT: u32 = 3;
pub const MAX_CONSECUTIVE_REPEATS: u32 = 5;

/// 组装入口（RΩ 按这个接，名字别自己发明）。
pub struct WorkerDeps {
    pub store: Arc<dyn SessionStore>,
    pub platform: Arc<dyn PlatformPort>,
    pub model: Arc<dyn ModelPort>,
    pub evidence: Arc<dyn EvidenceWriter>,
    pub config: AiteConfig,
    pub gateway: Option<Arc<dyn ToolGateway>>,
    pub sandbox: Option<Arc<dyn SandboxPort>>,
}

/// Worker（T2）。一个实例可以跑多个任务，每个任务的状态都在 `run()` 的栈上。
pub struct AgentWorker {
    store: Arc<dyn SessionStore>,
    platform: Arc<dyn PlatformPort>,
    model: Arc<dyn ModelPort>,
    evidence: Arc<dyn EvidenceWriter>,
    config: AiteConfig,
    gateway: Option<Arc<dyn ToolGateway>>,
    sandbox: Option<Arc<dyn SandboxPort>>,
    clock: Clock,
    sleep: Sleeper,
    /// 此刻在飞的任务（app 收尾时给硬取消的任务善终用）。每次 `_save` 刷新快照，
    /// 免得收尾拿到的是开跑那一刻的 steps=0。
    in_flight: Mutex<HashMap<String, (Task, Session)>>,
}

impl AgentWorker {
    pub fn new(deps: WorkerDeps) -> Self {
        Self {
            store: deps.store,
            platform: deps.platform,
            model: deps.model,
            evidence: deps.evidence,
            config: deps.config,
            gateway: deps.gateway,
            sandbox: deps.sandbox,
            clock: crate::default_clock(),
            sleep: crate::default_sleeper(),
            in_flight: Mutex::new(HashMap::new()),
        }
    }

    /// 测试注入：单调钟（秒）。卡片 500ms 窗口与 max_wall_sec 都读它。
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// 测试注入：退避用的 sleep。不真等，测试里把钟拨过去即可。
    pub fn with_sleep(mut self, sleep: Sleeper) -> Self {
        self.sleep = sleep;
        self
    }

    // ---- 主循环 --------------------------------------------------------

    async fn agent_loop(&self, ctx: &mut RunContext, hooks: &RunHooks) -> Result<Task, RunError> {
        let mut messages = self.build_messages(ctx).await?;
        if ctx.task.title.is_empty() {
            let last = last_user_text(&messages);
            let raw = if last.is_empty() {
                "处理中"
            } else {
                last.as_str()
            };
            ctx.task.title = clip(raw, MAX_TITLE_CHARS);
        }
        ctx.task.status = TaskStatus::Planning;
        // 覆盖建任务时写进去的 config 值：真正跑这一趟的是哪个模型以它为准
        ctx.task.model = self.model.name();
        self.save(ctx).await?;

        let started = (self.clock)();
        loop {
            if (hooks.is_cancelled)() {
                return self.cancel(ctx).await;
            }
            if ctx.task.steps >= ctx.task.max_steps {
                let text = texts::step_limit(&ctx.task.task_no);
                return self.fail(ctx, &text).await;
            }
            if (self.clock)() - started >= f64::from(ctx.task.max_wall_sec) {
                let text = texts::wall_limit(&ctx.task.task_no);
                return self.fail(ctx, &text).await;
            }

            // R6 排队进来的 steer 消息，在每步开始前合并进上下文
            for text in (hooks.drain_steer)() {
                messages.push(Message::text(Role::User, text));
            }

            let step_index = ctx.task.steps;
            let turn = match self.chat(&messages).await {
                Ok(turn) => turn,
                Err(err) => {
                    tracing::error!(task = %ctx.task.id, %err, "worker.model_failed");
                    let text = texts::model_unavailable(&ctx.task.task_no);
                    return self.fail(ctx, &text).await;
                }
            };

            ctx.task.steps += 1;
            ctx.task.tokens_in += turn.usage.input_tokens;
            ctx.task.tokens_out += turn.usage.output_tokens;
            ctx.task.cost += self.price(turn.usage.input_tokens, turn.usage.output_tokens);
            // 注意：hash 算的是**这次 chat 的输入**，本次回复还没进 messages
            let payload = json!({
                "model": self.model.name(),
                "step": step_index,
                "messages_hash": messages_hash(&messages),
                "usage": turn.usage,
                "finish_reason": turn.finish_reason,
            });
            self.append_evidence(&ctx.task.id, EvidenceKind::ModelCall, payload)
                .await?;

            let calls = turn.message.tool_calls.clone().unwrap_or_default();
            let content = turn.message.content.clone();
            messages.push(turn.message);

            if calls.is_empty() {
                let text = content.trim().to_string();
                // §3.3 只返回文本：仅第一步兜底成 final，之后提示它用工具
                if step_index == 0 && !text.is_empty() {
                    return self.deliver(ctx, &text, &[], true).await;
                }
                messages.push(Message::text(Role::System, texts::NUDGE_TEXT));
                ctx.card.maybe_flush().await?;
                continue;
            }

            for call in &calls {
                // 只认**连续**相同：中间插进别的调用说明模型还在换招，不算卡住。
                // 一步出多张牌时按 tool_calls 的顺序逐张算 —— 同一步里出两张一样的牌算 2 次，
                // 那比隔了一步再重复更卡。两条都跟观测侧的 repeat_loops 一个口径。
                let sig = fingerprint::call_signature(call);
                ctx.repeats = if ctx.last_call_sig.as_deref() == Some(sig.as_str()) {
                    ctx.repeats + 1
                } else {
                    1
                };
                ctx.last_call_sig = Some(sig);
                // 本地工具照样进计数（口径要和观测侧对得上），但不由它开火：连发 checklist_*
                // 是 §3.8 08_step_limit 已经钉住的 max_steps 那条路，抢在前面接会把那条规格
                // 声明在真实 max_steps=40 下变成假的。
                let spinning = !is_local_tool(&call.name);

                if spinning && ctx.repeats >= MAX_CONSECUTIVE_REPEATS {
                    // 在执行**之前**就收：前 4 次结果一模一样，第 5 次没有再跑一遍的必要，
                    // 尤其 run_python 那次还要再起一次沙箱执行。
                    let text = texts::repeat_failure(
                        &ctx.task.task_no,
                        MAX_CONSECUTIVE_REPEATS,
                        &call.name,
                    );
                    return self.fail(ctx, &text).await;
                }

                let is_final = call.name == final_tool().name;
                if !is_final {
                    self.ensure_card(ctx).await?; // W3
                }

                if is_final {
                    self.append_evidence(
                        &ctx.task.id,
                        EvidenceKind::ToolCall,
                        json!({"call_id": call.call_id, "name": call.name, "arguments": call.arguments}),
                    )
                    .await?;
                    match parse_final(call) {
                        Err(bad) => {
                            self.local_result(ctx, call, false, bad.content, Some(bad.code))
                                .await?;
                            messages.push(tool_message(call, bad.content));
                            ctx.invalid_args += 1;
                            break;
                        }
                        Ok((reply, artifacts)) => {
                            let answering = !ctx.card.sent();
                            return self.deliver(ctx, &reply, &artifacts, answering).await;
                        }
                    }
                }

                let outcome = self.run_tool(ctx, call).await?;
                messages.push(tool_message(call, &outcome.content));
                if spinning && ctx.repeats == REPEAT_NUDGE_AT {
                    messages.push(Message::text(
                        Role::System,
                        texts::repeat_nudge(ctx.repeats, &call.name),
                    ));
                }

                if outcome.error_code == Some(ToolErrorCode::InvalidArgs) {
                    ctx.invalid_args += 1;
                } else if outcome.ok {
                    ctx.invalid_args = 0;
                }
                if outcome.error_code == Some(ToolErrorCode::Sandbox) {
                    ctx.sandbox_errors += 1;
                } else if outcome.ok {
                    ctx.sandbox_errors = 0;
                }

                if ctx.invalid_args >= MAX_CONSECUTIVE_INVALID_ARGS {
                    let text = texts::invalid_args_failure(
                        &ctx.task.task_no,
                        MAX_CONSECUTIVE_INVALID_ARGS,
                    );
                    return self.fail(ctx, &text).await;
                }
                if ctx.sandbox_errors >= MAX_CONSECUTIVE_SANDBOX_ERRORS {
                    let text =
                        texts::sandbox_failure(&ctx.task.task_no, MAX_CONSECUTIVE_SANDBOX_ERRORS);
                    return self.fail(ctx, &text).await;
                }
            }

            if ctx.invalid_args >= MAX_CONSECUTIVE_INVALID_ARGS {
                let text =
                    texts::invalid_args_failure(&ctx.task.task_no, MAX_CONSECUTIVE_INVALID_ARGS);
                return self.fail(ctx, &text).await;
            }
            self.refresh_card(ctx, CardStatus::Working).await?;
            self.save(ctx).await?;
        }
    }

    // ---- 上下文与模型 --------------------------------------------------

    async fn build_messages(&self, ctx: &mut RunContext) -> Result<Vec<Message>, RunError> {
        let turns = self
            .store
            .list_turns(&ctx.session.id, TRANSCRIPT_LIMIT)
            .await?;
        let history = match self
            .platform
            .read_history(
                &ctx.session.chat_id,
                self.config.feishu.history_window,
                None,
            )
            .await
        {
            Ok(h) => h,
            Err(err) => {
                tracing::error!(chat = %ctx.session.chat_id, %err, "worker.read_history_failed");
                Vec::new()
            }
        };
        // 附件取「最后一个有附件的 turn」
        let attachments = turns
            .iter()
            .rev()
            .find(|t| !t.attachments.is_empty())
            .map(|t| t.attachments.clone())
            .unwrap_or_default();
        ctx.attachments_message_id = Some(match attachments.first() {
            Some(a) => a.message_id.clone(),
            None => ctx.session.anchor.message_id.clone(),
        });
        let prompt = load_system_prompt(Path::new(&self.config.worker.system_prompt_path))?;
        Ok(build_context(&prompt, &turns, &history, &attachments))
    }

    async fn chat(&self, messages: &[Message]) -> Result<ModelTurn, ModelError> {
        let mut last: Option<ModelError> = None;
        for delay in [0.0, MODEL_RETRY_DELAYS[0], MODEL_RETRY_DELAYS[1]] {
            if delay > 0.0 {
                (self.sleep)(delay).await;
            }
            match self
                .model
                .chat(
                    messages,
                    all_model_tools(),
                    self.config.model.max_tokens,
                    self.config.model.temperature,
                )
                .await
            {
                Ok(turn) => return Ok(turn),
                Err(err) => last = Some(err),
            }
        }
        Err(last.unwrap_or_else(|| ModelError::Upstream("模型调用没有产生结果".into())))
    }

    fn price(&self, tokens_in: u64, tokens_out: u64) -> f64 {
        let m = &self.config.model;
        (tokens_in as f64 * m.price_in_per_mtok + tokens_out as f64 * m.price_out_per_mtok)
            / 1_000_000.0
    }

    // ---- 工具分发（W2）-------------------------------------------------

    async fn run_tool(
        &self,
        ctx: &mut RunContext,
        call: &ToolCallRequest,
    ) -> Result<ToolOutcome, RunError> {
        if is_local_tool(&call.name) {
            self.run_local_tool(ctx, call).await
        } else {
            self.run_gateway_tool(ctx, call).await
        }
    }

    async fn run_local_tool(
        &self,
        ctx: &mut RunContext,
        call: &ToolCallRequest,
    ) -> Result<ToolOutcome, RunError> {
        let args = &call.arguments;
        self.append_evidence(
            &ctx.task.id,
            EvidenceKind::ToolCall,
            json!({"call_id": call.call_id, "name": call.name, "arguments": args}),
        )
        .await?;

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
                // "checklist_check"[10:] == "check"；"checklist_fail"[10:] == "fail"
                let op = &call.name[10..];
                self.checklist_evidence(&ctx.task.id, op, json!({"id": item_id, "state": state}))
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

    async fn local_result(
        &self,
        ctx: &RunContext,
        call: &ToolCallRequest,
        ok: bool,
        content: &str,
        code: Option<ToolErrorCode>,
    ) -> Result<ToolOutcome, RunError> {
        self.append_evidence(
            &ctx.task.id,
            EvidenceKind::ToolResult,
            json!({
                "call_id": call.call_id,
                "name": call.name,
                "ok": ok,
                "error": code.map(|c| c.as_str()),
                "content_hash": sha256_hex(content.as_bytes()),
                "duration_ms": 0,
            }),
        )
        .await?;
        Ok(ToolOutcome {
            ok,
            content: content.to_string(),
            error_code: code,
        })
    }

    async fn checklist_evidence(
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

    async fn run_gateway_tool(
        &self,
        ctx: &mut RunContext,
        call: &ToolCallRequest,
    ) -> Result<ToolOutcome, RunError> {
        self.append_evidence(
            &ctx.task.id,
            EvidenceKind::ToolCall,
            json!({"call_id": call.call_id, "name": call.name, "arguments": call.arguments}),
        )
        .await?;
        let Some(gateway) = self.gateway.clone() else {
            let content = texts::tool_unavailable(&call.name);
            return self
                .local_result(ctx, call, false, &content, Some(ToolErrorCode::NotFound))
                .await;
        };

        let result = gateway.call(&self.tool_context(ctx), call).await;
        let code = result.error.as_ref().map(|e| e.code);
        self.append_evidence(
            &ctx.task.id,
            EvidenceKind::ToolResult,
            json!({
                "call_id": result.call_id,
                "name": result.name,
                "ok": result.ok,
                "error": code.map(|c| c.as_str()),
                "content_hash": sha256_hex(result.content.as_bytes()),
                "duration_ms": result.duration_ms,
            }),
        )
        .await?;
        let mut content = result.content;
        if !result.ok
            && content.is_empty()
            && let Some(err) = &result.error
        {
            content = texts::tool_error_content(err.code.as_str(), &err.message);
        }
        Ok(ToolOutcome {
            ok: result.ok,
            content,
            error_code: code,
        })
    }

    fn tool_context(&self, ctx: &RunContext) -> ToolContext {
        ToolContext {
            tenant_id: ctx.session.tenant_id.clone(),
            workspace_id: ctx.session.workspace_id.clone(),
            chat_id: ctx.session.chat_id.clone(),
            session_id: ctx.session.id.clone(),
            task_id: ctx.task.id.clone(),
            session_token: ctx.task.session_token.clone(),
            thread_id: ctx.session.anchor.thread_id.clone(),
            attachments_message_id: ctx.attachments_message_id.clone(),
        }
    }

    // ---- 卡片（W3 / W4）------------------------------------------------

    async fn ensure_card(&self, ctx: &mut RunContext) -> Result<(), RunError> {
        if ctx.card.sent() {
            return Ok(());
        }
        ctx.task.status = TaskStatus::Working;
        let card = render_card(
            &ctx.task,
            &ctx.session,
            &ctx.initiator,
            CardStatus::Working,
            ctx.note.as_deref(),
        );
        let chat_id = ctx.session.chat_id.clone();
        let reply_to = ctx.thread_root();
        ctx.card
            .ensure_card(&chat_id, Some(reply_to.as_str()), &card)
            .await?;
        ctx.task.card_id = ctx.card.card_id().map(str::to_string);
        self.save(ctx).await
    }

    async fn refresh_card(&self, ctx: &mut RunContext, status: CardStatus) -> Result<(), RunError> {
        if !ctx.card.sent() {
            return Ok(());
        }
        let card = render_card(
            &ctx.task,
            &ctx.session,
            &ctx.initiator,
            status,
            ctx.note.as_deref(),
        );
        ctx.card.update(card).await?;
        Ok(())
    }

    async fn close_card(&self, ctx: &mut RunContext, status: CardStatus) -> Result<(), RunError> {
        if !ctx.card.sent() {
            return Ok(());
        }
        let card = render_card(
            &ctx.task,
            &ctx.session,
            &ctx.initiator,
            status,
            ctx.note.as_deref(),
        );
        ctx.card.force_flush(Some(card)).await?;
        Ok(())
    }

    // ---- 收尾 ----------------------------------------------------------

    /// W5：产物逐个 get_file → send_file → evidence artifact；然后 send_text；delivered；finalize。
    async fn deliver(
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
        self.close_card(ctx, CardStatus::Delivered).await?;
        self.finish(ctx).await?;
        Ok(ctx.task.clone())
    }

    async fn fail(&self, ctx: &mut RunContext, text: &str) -> Result<Task, RunError> {
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
        self.append_evidence(
            &ctx.task.id,
            EvidenceKind::Failed,
            json!({"reason": text, "steps": ctx.task.steps}),
        )
        .await?;
        self.close_card(ctx, CardStatus::Failed).await?;
        self.finish(ctx).await?;
        Ok(ctx.task.clone())
    }

    async fn cancel(&self, ctx: &mut RunContext) -> Result<Task, RunError> {
        ctx.task.status = TaskStatus::Cancelled;
        self.append_evidence(
            &ctx.task.id,
            EvidenceKind::Cancelled,
            json!({"steps": ctx.task.steps}),
        )
        .await?;
        self.close_card(ctx, CardStatus::Cancelled).await?;
        self.finish(ctx).await?;
        Ok(ctx.task.clone())
    }

    async fn finish(&self, ctx: &mut RunContext) -> Result<(), RunError> {
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

    async fn save(&self, ctx: &mut RunContext) -> Result<(), RunError> {
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
    async fn fetch_artifact(&self, ctx: &mut RunContext, path: &str) -> Option<Vec<u8>> {
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

    async fn append_evidence(
        &self,
        task_id: &str,
        kind: EvidenceKind,
        payload: Value,
    ) -> Result<(), RunError> {
        self.evidence
            .append(task_id, kind, json_object(payload))
            .await?;
        Ok(())
    }
}

#[async_trait]
impl aite_contracts::TaskWorker for AgentWorker {
    async fn run(
        &self,
        task: Task,
        session: Session,
        initiator: Option<String>,
        hooks: RunHooks,
    ) -> Task {
        let task_id = task.id.clone();
        // 令牌校验是失败关闭的：不登记则该任务的每个工具调用都 denied
        if let Some(gateway) = &self.gateway {
            gateway.register_task(&task.id, &task.session_token);
        }
        if let Ok(mut live) = self.in_flight.lock() {
            live.insert(task_id.clone(), (task.clone(), session.clone()));
        }

        let initiator = initiator
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| session.created_by.clone());
        let mut ctx = RunContext {
            card: CardCoalescer::new(
                self.platform.clone(),
                self.config.worker.card_update_min_interval_ms,
                self.clock.clone(),
            ),
            task,
            session,
            initiator,
            note: None,
            invalid_args: 0,
            sandbox_errors: 0,
            repeats: 0,
            last_call_sig: None,
            attachments_message_id: None,
        };

        let out = match self.agent_loop(&mut ctx, &hooks).await {
            Ok(task) => task,
            // §3.3 任何未捕获的失败 → task failed + 回帖 + evidence，进程不退出
            Err(err) => {
                tracing::error!(task = %task_id, %err, "worker.unhandled");
                let text = texts::run_error(&ctx.task.task_no, &err.to_string());
                match self.fail(&mut ctx, &text).await {
                    Ok(task) => task,
                    Err(err2) => {
                        // 收尾本身也塌了：状态还是要落到 failed，不能把错误抛回控制面
                        tracing::error!(task = %task_id, err = %err2, "worker.fail_path_failed");
                        let mut t = ctx.task.clone();
                        t.status = TaskStatus::Failed;
                        t
                    }
                }
            }
        };

        if let Ok(mut live) = self.in_flight.lock() {
            live.remove(&task_id);
        }
        out
    }

    fn in_flight(&self) -> Vec<(Task, Session)> {
        match self.in_flight.lock() {
            Ok(live) => live.values().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// `store.list_turns` 的上限：W1 先砍到最近 200 轮，再按 40 轮规则截断。
const TRANSCRIPT_LIMIT: u32 = 200;

/// 一次 `run()` 的可变状态。放一个结构体里免得在方法间传七八个参数。
struct RunContext {
    task: Task,
    session: Session,
    card: CardCoalescer,
    initiator: String,
    note: Option<String>,
    invalid_args: u32,
    sandbox_errors: u32,
    /// 上一张牌的指纹，以及它已经连着出了几次（跨步保留：33 次重复是一步一次出来的）
    repeats: u32,
    last_call_sig: Option<String>,
    attachments_message_id: Option<String>,
}

impl RunContext {
    fn thread_root(&self) -> String {
        self.session
            .anchor
            .thread_id
            .clone()
            .unwrap_or_else(|| self.session.anchor.message_id.clone())
    }
}

/// 本地工具的执行结果，形状对齐 `ToolResult` 里 worker 关心的那几项。
struct ToolOutcome {
    ok: bool,
    content: String,
    error_code: Option<ToolErrorCode>,
}

struct FinalError {
    content: &'static str,
    code: ToolErrorCode,
}

fn parse_final(call: &ToolCallRequest) -> Result<(String, Vec<Map<String, Value>>), FinalError> {
    let args = &call.arguments;
    let reply = args.get("reply").and_then(Value::as_str);
    let Some(reply) = reply.filter(|r| !r.trim().is_empty()) else {
        return Err(FinalError {
            content: texts::FINAL_REPLY_REQUIRED,
            code: ToolErrorCode::InvalidArgs,
        });
    };
    let artifacts = match args.get("artifacts") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_object)
            .filter(|m| m.get("path").is_some_and(is_truthy))
            .cloned()
            .collect(),
        // Python 是 `raw = args.get("artifacts") or []`（loop.py:634）：**假值**
        // （`null` / `""` / `{}` / `0` / `false` / `[]`）一律当成「没有产物」照常交付，
        // 只有**真值但不是数组**才判 invalid_args。
        // 这条曾经是 Rust 侧的行为翻转：弱模型给 `artifacts: ""` 不罕见，那时 Python
        // 交付、Rust 退回重来 —— 而这是**交付路径**，退回去的代价是用户什么都收不到。
        Some(v) if !is_truthy(v) => Vec::new(),
        Some(_) => {
            return Err(FinalError {
                content: texts::FINAL_ARTIFACTS_MUST_BE_ARRAY,
                code: ToolErrorCode::InvalidArgs,
            });
        }
    };
    Ok((reply.to_string(), artifacts))
}

/// Python 的真值语义：空串 / 0 / false / null / 空容器都是假。
fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// 对齐 Python 的 `str(art.get(key, ""))`：字符串取原文，缺省取空串，别的取 JSON 形态。
fn plain_string(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

fn tool_message(call: &ToolCallRequest, content: &str) -> Message {
    Message {
        role: Role::Tool,
        content: content.to_string(),
        tool_calls: None,
        tool_call_id: Some(call.call_id.clone()),
        name: Some(call.name.clone()),
    }
}

fn last_user_text(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User && !m.content.is_empty())
        .map(|m| m.content.clone())
        .unwrap_or_default()
}

/// W8：`model_call` 只留输入消息的 hash，正文不进证据。
fn messages_hash(messages: &[Message]) -> String {
    let joined = messages
        .iter()
        .map(|m| serde_json::to_string(m).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    sha256_hex(joined.as_bytes())
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn json_object(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}
