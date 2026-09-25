//! Agent Loop（§3.6 W1–W9）。对应旧 `aite/worker/loop.py` + `aite/app.py::AppWorker`。
//!
//! 一步 = 一次 `ModelPort::chat`。模型只能通过工具驱动进度面与产出：
//! checklist_* 与 final 由 worker 本地处理，其余交 `ToolGateway::call`（W2）。
//!
//! 旧版 `AppWorker` 只多做两件事（登记 session_token、记 in_flight），Rust 版并进来 ——
//! 冻结的 `ToolGateway` 已经显式化了 `register_task / release_task`，没必要再套一层。
//!
//! CC3 起执行面按职责拆开（零行为变化，原样搬）：本文件留结构体、主循环、上下文与模型调用、
//! 工具分发骨架与小工具；收尾在 `deliver.rs`，卡片三件在 `card.rs`，本地工具在 `local_tools/`。
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use aite_contracts::{
    AiteConfig, CardStatus, EvidenceKind, EvidenceWriter, Message, ModelError, ModelPort,
    ModelTurn, PlatformPort, Role, RunHooks, SandboxPort, Session, SessionStore, Task, TaskStatus,
    ToolCallRequest, ToolContext, ToolErrorCode, ToolGateway, ToolSpec, checklist_tools,
    final_tool, gateway_tools,
};
use async_trait::async_trait;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::card::{CardCoalescer, MAX_TITLE_CHARS, clip};
use crate::context::{BlockCtx, assemble, load_system_prompt};
use crate::deps::WorkerDeps;
use crate::local_tools::{self, parse_final};
use crate::{Clock, RunError, Sleeper, budget, fingerprint, texts};

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

/// `tool_result` 证据里 `content_summary` 的长度上限（字符数，`clip` 会折叠空白）。
///
/// 为什么是 200：`Task.result_summary` 用的就是这个数（本文件 `deliver()` 里的
/// `clip(reply, 200)`），排障时两处长度一致好对照。够装下一条 Python traceback 的
/// 最后一行（`SandboxError` / `ModuleNotFoundError: No module named 'xxx'` 这种），
/// 而那正是 §8 第 6 条说「排 M3 的沙箱问题时最疼」的那一行。
///
/// **摘要不做脱敏，也没有任何脱敏器认得它** —— `preflight` 的 `Redactor` 只在起飞
/// 自检那条路上、只认配置里的环境变量名，根本不在证据这条路上；`evidence show` 的
/// `redact()` 是**渲染时**按键名打码的，只作用在 `tool_call` 的 `arguments` 上。
/// 这不是新开的口子：`tool_call` 证据早就把 `arguments` **全文**原样存进去了，
/// 摘要存的是同一趟调用的输出侧，风险面没有变大。真要收紧，该收的是整个证据面的
/// 写入侧脱敏，那是单独一件事（见回执「记账转出去的」）。
pub const MAX_TOOL_SUMMARY_CHARS: usize = 200;

/// Worker（T2）。一个实例可以跑多个任务，每个任务的状态都在 `run()` 的栈上。
pub struct AgentWorker {
    pub(crate) store: Arc<dyn SessionStore>,
    pub(crate) platform: Arc<dyn PlatformPort>,
    pub(crate) model: Arc<dyn ModelPort>,
    pub(crate) evidence: Arc<dyn EvidenceWriter>,
    pub(crate) config: AiteConfig,
    pub(crate) gateway: Option<Arc<dyn ToolGateway>>,
    pub(crate) sandbox: Option<Arc<dyn SandboxPort>>,
    pub(crate) clock: Clock,
    pub(crate) sleep: Sleeper,
    /// 此刻在飞的任务（app 收尾时给硬取消的任务善终用）。每次 `_save` 刷新快照，
    /// 免得收尾拿到的是开跑那一刻的 steps=0。
    pub(crate) in_flight: Mutex<HashMap<String, (Task, Session)>>,
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

    pub(crate) async fn agent_loop(
        &self,
        ctx: &mut RunContext,
        hooks: &RunHooks,
    ) -> Result<Task, RunError> {
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
            // EE3 的挂点（CC3 预埋，本轨恒 None）
            if let Some(text) = budget::before_step(&ctx.task).await {
                return self.fail(ctx, &text).await;
            }

            // R6 排队进来的 steer 消息，在每步开始前合并进上下文
            for text in (hooks.drain_steer)() {
                messages.push(Message::text(Role::User, text));
            }

            let step_index = ctx.task.steps;
            let turn = match self.chat(ctx, &messages).await {
                Ok(turn) => turn,
                Err(err) => {
                    tracing::error!(task = %ctx.task.id, %err, "worker.model_failed");
                    let text = texts::model_unavailable(&ctx.task.task_no);
                    return self.fail(ctx, &text).await;
                }
            };
            // EE3 的挂点（CC3 预埋，本轨空实现）
            budget::after_model_call(&ctx.task, &turn.usage).await;

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
                let spinning = !local_tools::is_local(&call.name);

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

    pub(crate) async fn build_messages(
        &self,
        ctx: &mut RunContext,
    ) -> Result<Vec<Message>, RunError> {
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
        let blocks = BlockCtx {
            session: &ctx.session,
            task: &ctx.task,
            turns: &turns,
            history: &history,
        };
        Ok(assemble(&prompt, &turns, &history, &attachments, &blocks).await)
    }

    /// 进模型的工具目录（CC3 ②，全仓唯一的目录行）：本地的 checklist_* 与 final、启用的额外本地工具、
    /// 再加 `gateway.catalog(ctx)`。没有 gateway 时退回契约里的 `gateway_tools()`（今天的 10 个）。
    /// 顺序与 `all_model_tools()` 一致（checklist → final → gateway）。
    pub(crate) fn tool_catalog(&self, ctx: &RunContext) -> Vec<ToolSpec> {
        let mut tools: Vec<ToolSpec> = checklist_tools().to_vec();
        tools.push(final_tool().clone());
        tools.extend(local_tools::enabled_specs());
        match &self.gateway {
            Some(gateway) => tools.extend(gateway.catalog(&self.tool_context(ctx))),
            None => tools.extend(gateway_tools().iter().cloned()),
        }
        tools
    }

    pub(crate) async fn chat(
        &self,
        ctx: &RunContext,
        messages: &[Message],
    ) -> Result<ModelTurn, ModelError> {
        let tools = self.tool_catalog(ctx);
        let mut last: Option<ModelError> = None;
        for delay in [0.0, MODEL_RETRY_DELAYS[0], MODEL_RETRY_DELAYS[1]] {
            if delay > 0.0 {
                (self.sleep)(delay).await;
            }
            match self
                .model
                .chat(
                    messages,
                    &tools,
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

    pub(crate) fn price(&self, tokens_in: u64, tokens_out: u64) -> f64 {
        let m = &self.config.model;
        (tokens_in as f64 * m.price_in_per_mtok + tokens_out as f64 * m.price_out_per_mtok)
            / 1_000_000.0
    }

    // ---- 工具分发（W2）-------------------------------------------------

    pub(crate) async fn run_tool(
        &self,
        ctx: &mut RunContext,
        call: &ToolCallRequest,
    ) -> Result<ToolOutcome, RunError> {
        if local_tools::is_local(&call.name) {
            self.run_local_tool(ctx, call).await
        } else {
            self.run_gateway_tool(ctx, call).await
        }
    }

    pub(crate) async fn local_result(
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
                "content_summary": clip(content, MAX_TOOL_SUMMARY_CHARS),
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

    pub(crate) async fn run_gateway_tool(
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
        // BB2 ③：沙箱 id 进证据。Gateway 是在这次调用里按需 acquire 的，所以要在
        // **调用之后**问它 —— 调用前问只会拿到 None。没有沙箱的工具（发消息、读历史）
        // 这里是 null，不是漏记。有了它，「哪个容器该收没收」就不用靠时间先后猜。
        let sandbox_id = gateway.sandbox_id_of(&ctx.task.id).await;
        self.append_evidence(
            &ctx.task.id,
            EvidenceKind::ToolResult,
            json!({
                "call_id": result.call_id,
                "name": result.name,
                "ok": result.ok,
                "error": code.map(|c| c.as_str()),
                "content_hash": sha256_hex(result.content.as_bytes()),
                // BB2 ③：摘要。原来失败时证据里只有 `error` 的错误码，拿不到那一行
                // 具体的报错文本 —— 排沙箱问题时最疼的就是这个。
                // **摘要不进 `content_hash`**：那个 hash 是对 content 全文算的，
                // 摘要只是同一份 content 的另一个视图，进去了就变成「改摘要即改哈希」。
                "content_summary": clip(&result.content, MAX_TOOL_SUMMARY_CHARS),
                "sandbox_id": sandbox_id,
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

    pub(crate) fn tool_context(&self, ctx: &RunContext) -> ToolContext {
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

    pub(crate) async fn append_evidence(
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
                self.evidence.clone(),
                task.id.clone(),
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
pub(crate) struct RunContext {
    pub(crate) task: Task,
    pub(crate) session: Session,
    pub(crate) card: CardCoalescer,
    pub(crate) initiator: String,
    pub(crate) note: Option<String>,
    pub(crate) invalid_args: u32,
    pub(crate) sandbox_errors: u32,
    /// 上一张牌的指纹，以及它已经连着出了几次（跨步保留：33 次重复是一步一次出来的）
    pub(crate) repeats: u32,
    pub(crate) last_call_sig: Option<String>,
    pub(crate) attachments_message_id: Option<String>,
}

impl RunContext {
    pub(crate) fn thread_root(&self) -> String {
        self.session
            .anchor
            .thread_id
            .clone()
            .unwrap_or_else(|| self.session.anchor.message_id.clone())
    }
}

/// 本地工具的执行结果，形状对齐 `ToolResult` 里 worker 关心的那几项。
pub(crate) struct ToolOutcome {
    pub(crate) ok: bool,
    pub(crate) content: String,
    pub(crate) error_code: Option<ToolErrorCode>,
}

pub(crate) fn tool_message(call: &ToolCallRequest, content: &str) -> Message {
    Message {
        role: Role::Tool,
        content: content.to_string(),
        tool_calls: None,
        tool_call_id: Some(call.call_id.clone()),
        name: Some(call.name.clone()),
    }
}

pub(crate) fn last_user_text(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User && !m.content.is_empty())
        .map(|m| m.content.clone())
        .unwrap_or_default()
}

/// W8：`model_call` 只留输入消息的 hash，正文不进证据。
pub(crate) fn messages_hash(messages: &[Message]) -> String {
    let joined = messages
        .iter()
        .map(|m| serde_json::to_string(m).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    sha256_hex(joined.as_bytes())
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

pub(crate) fn json_object(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}
