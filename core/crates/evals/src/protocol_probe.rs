//! 真模型出牌观测（对应旧 `aite/evals/protocol_probe.py`）。
//!
//! `--model live` 跑起来之后，`evals/p0/*.yaml` 里那些照 `model_script` 台词写的 `expect`
//! 必然大面积红 —— 那是预期的，不是 bug（判据钉的是 scripted 路径的 P0 验收）。真正要看的
//! 是另一件事：**真模型能不能按 `aite-contracts::protocol` 那套协议出牌，接不住的地方兜底
//! 够不够**。这个文件就是看这件事的眼睛。
//!
//! 装置分两半：
//!
//! * `ModelProbe`：`ModelPort` 形状的包装，包住任意模型（`FakeModel` 或真客户端），
//!   一次 `chat` 记一条 `Observation` —— 出了什么牌、参数合不合 `ToolSpec.parameters`、
//!   报没报错、离上一次隔了多久。
//! * `analyze()`：把观测跟 evidence 链、出站消息、任务终态对起来。
//!
//! **它只看，不改行为。**
//!
//! 出牌怎么切分成「轮」：worker 一个 task 一份 `messages`，全程只 append，且**每一步的
//! 第一件事就是把模型刚才那张回牌 append 进去**。所以续接的判据有两条，缺一不可：
//!
//! 1. 这次调用的前 N 条 == 上次调用的全部；
//! 2. 第 N 条（新长出来的头一条）就是上次那张 assistant 回牌。
//!
//! 只比前缀不够 —— 同一会话里的第二个 task，上下文是第一个 task 的**前缀扩展**
//! （多一条 user turn），只看第 1 条会把两个 task 并成一轮，02_thread_followup 就是。
//!
//! 重试（模型 5xx）原样再投同一份 messages：一条都没长，且上一发是报错的 ——
//! 单独认这一种，落在同一轮里。
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aite_contracts::{Message, ModelError, ModelPort, ModelTurn, ToolSpec, all_model_tools};
use aite_testing::recorder::CallLog;
use aite_testing::{kwargs, validate};
use async_trait::async_trait;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// 摘要里参数值截到多长 —— run_python 的 code 动辄几百字，全打出来没法读。
pub const MAX_ARG_CHARS: usize = 200;
/// 纯文本步骤在摘要里截到多长。
pub const MAX_TEXT_CHARS: usize = 160;
/// 同一张牌连着出几次才算「卡住了」。2 次多半还是正常探测（真模型实测里连着两次
/// `list_files()` 是常态），3 次起才是原地打转。
pub const MIN_REPEAT_RUN: usize = 3;

fn clip(text: &str, limit: usize) -> String {
    let text = text.replace('\n', "⏎");
    if text.chars().count() <= limit {
        return text;
    }
    let head: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{head}…")
}

fn stringify(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| format!("{other:?}")),
    }
}

/// 一次工具调用的指纹：工具名 + 顶层 key 排过序的 JSON。
///
/// 刻意和 worker 的 `_call_signature` 逐字同构 —— 观测侧说「打转了 33 次」而兜底侧说
/// 「没打转」的话，排查时两边会打架，所以「同一张牌」在两处必须指同一件事。
pub fn fingerprint(name: &str, arguments: &Map<String, Value>) -> String {
    let sorted: BTreeMap<&String, &Value> = arguments.iter().collect();
    let body = serde_json::to_string(&sorted).unwrap_or_else(|_| format!("{arguments:?}"));
    format!("{name}:{body}")
}

fn message_key(m: &Message) -> String {
    serde_json::to_string(m).unwrap_or_default()
}

/// 前 n 条 message 的指纹。用来判「这次调用是不是上次那一轮的续接」。
fn prefix_key(messages: &[Message], n: usize) -> String {
    let mut h = Sha256::new();
    for m in messages.iter().take(n) {
        h.update(message_key(m).as_bytes());
    }
    hex::encode(h.finalize())
}

fn row(m: &Message) -> Value {
    let mut out = kwargs! {
        "role" => json!(m.role.as_str()),
        "content" => json!(clip(&m.content, MAX_TEXT_CHARS)),
    };
    if let Some(calls) = &m.tool_calls
        && !calls.is_empty()
    {
        out.insert(
            "tool_calls".to_string(),
            json!(calls.iter().map(|c| c.name.clone()).collect::<Vec<_>>()),
        );
    }
    if let Some(name) = &m.name {
        out.insert("name".to_string(), json!(name));
    }
    Value::Object(out)
}

/// 模型出的一张牌，外加「这张牌合不合法」的判定。
#[derive(Debug, Clone)]
pub struct ToolCallObservation {
    pub call_id: String,
    pub name: String,
    pub arguments: Map<String, Value>,
    /// 名字在不在 all_model_tools() 里
    pub in_protocol: bool,
    /// 按 ToolSpec.parameters 校验的结果；协议外的工具没有 spec，记 None
    pub schema_ok: Option<bool>,
    pub schema_error: Option<String>,
}

impl ToolCallObservation {
    fn clipped_arguments(&self) -> Value {
        let mut out = Map::new();
        for (k, v) in &self.arguments {
            out.insert(k.clone(), json!(clip(&stringify(v), MAX_ARG_CHARS)));
        }
        Value::Object(out)
    }

    pub fn to_json(&self) -> Value {
        let mut out = kwargs! {
            "name" => json!(self.name),
            "call_id" => json!(self.call_id),
            "arguments" => self.clipped_arguments(),
            "in_protocol" => json!(self.in_protocol),
        };
        if let Some(ok) = self.schema_ok {
            out.insert("schema_ok".to_string(), json!(ok));
        }
        if let Some(err) = &self.schema_error {
            out.insert("schema_error".to_string(), json!(err));
        }
        Value::Object(out)
    }
}

/// 一次 `ModelPort::chat` 的全部可见事实。失败的那次也记 —— 重试要靠它数。
#[derive(Debug, Clone, Default)]
pub struct Observation {
    /// 第几次 chat（全局，含失败重试）
    pub index: usize,
    /// 第几轮对话；worker 一个 task 一轮
    pub run: usize,
    /// 本轮里的第几次调用（含失败重试）
    pub attempt: usize,
    pub n_messages: usize,
    /// 相对上一次新增的 message
    pub delta: Vec<Value>,
    pub elapsed_ms: u64,
    /// 距上一次 chat 返回过了多久（看重试间隔）
    pub gap_ms: u64,
    pub error: Option<String>,
    pub text: String,
    pub finish_reason: String,
    pub tool_calls: Vec<ToolCallObservation>,
    pub usage: Map<String, Value>,
}

impl Observation {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }

    /// 那条「既无 tool_call 也无 final，只返回文本」的出牌。
    pub fn text_only(&self) -> bool {
        self.ok() && self.tool_calls.is_empty()
    }

    pub fn to_json(&self) -> Value {
        let mut out = kwargs! {
            "index" => json!(self.index),
            "run" => json!(self.run),
            "attempt" => json!(self.attempt),
            "n_messages" => json!(self.n_messages),
            "elapsed_ms" => json!(self.elapsed_ms),
            "gap_ms" => json!(self.gap_ms),
        };
        if let Some(err) = &self.error {
            out.insert("error".to_string(), json!(err));
            return Value::Object(out);
        }
        out.insert("finish_reason".to_string(), json!(self.finish_reason));
        if !self.text.is_empty() {
            out.insert("text".to_string(), json!(clip(&self.text, MAX_TEXT_CHARS)));
        }
        out.insert(
            "tool_calls".to_string(),
            json!(
                self.tool_calls
                    .iter()
                    .map(ToolCallObservation::to_json)
                    .collect::<Vec<_>>()
            ),
        );
        if !self.usage.is_empty() {
            out.insert("usage".to_string(), Value::Object(self.usage.clone()));
        }
        if !self.delta.is_empty() {
            out.insert("delta".to_string(), Value::Array(self.delta.clone()));
        }
        Value::Object(out)
    }
}

#[derive(Default)]
struct ProbeState {
    observations: Vec<Observation>,
    prev_len: usize,
    prev_key: String,
    /// 上一次返回的那条 assistant 的指纹
    prev_reply: Option<String>,
    run: i64,
    attempt: usize,
    last_return: Option<Instant>,
}

/// `ModelPort` 的透明包装：照原样转发 `chat`，顺手把每一步出牌记下来。
///
/// Rust 没有 Python 的 `__getattr__` 透传，所以被包的模型的额外方法
/// （`FakeModel::release_holds()` 之类）由调用方自己留一份 `Arc` 去用；
/// 探针只补 `ModelPort` 之外的记账面（`calls` / `call_count` / `in_flight`）。
pub struct ModelProbe {
    pub inner: Arc<dyn ModelPort>,
    pub name: String,
    pub calls: CallLog,
    specs: BTreeMap<String, ToolSpec>,
    state: Mutex<ProbeState>,
    in_flight: AtomicUsize,
    last_failed: AtomicBool,
}

impl ModelProbe {
    pub fn new(inner: Arc<dyn ModelPort>) -> Self {
        let name = inner.name();
        Self {
            inner,
            name,
            calls: CallLog::new(),
            specs: all_model_tools()
                .iter()
                .map(|t| (t.name.clone(), t.clone()))
                .collect(),
            state: Mutex::new(ProbeState {
                run: -1,
                ..ProbeState::default()
            }),
            in_flight: AtomicUsize::new(0),
            last_failed: AtomicBool::new(false),
        }
    }

    pub fn observations(&self) -> Vec<Observation> {
        self.state
            .lock()
            .expect("ModelProbe 锁")
            .observations
            .clone()
    }

    /// 有几次 chat 还没返回。
    ///
    /// `Deps::activity()` 是个「变没变」的探测器，看不见长时间的 await —— 真模型一次
    /// 调用动辄几秒，期间一个替身都不会被碰。`settle()` 靠这个数才不会把
    /// 「正在等模型回包」当成「系统不干活了」。
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// 上一发是报错的：worker 可能正睡在退避里，等着再来一发。
    pub fn awaiting_retry(&self) -> bool {
        self.last_failed.load(Ordering::SeqCst)
    }

    pub fn call_count(&self) -> usize {
        self.calls.count("chat")
    }

    /// 模型出过的所有 tool_call 名字（含本地 checklist_* 与 final）。
    pub fn tool_names_emitted(&self) -> Vec<String> {
        self.observations()
            .iter()
            .flat_map(|o| o.tool_calls.iter().map(|tc| tc.name.clone()))
            .collect()
    }

    // ---- 内部 -----------------------------------------------------------

    /// 这次调用是不是上一轮的续接。判据见模块 docstring。
    fn is_continuation(state: &ProbeState, messages: &[Message]) -> bool {
        if state.prev_len == 0 || messages.len() < state.prev_len {
            return false;
        }
        if prefix_key(messages, state.prev_len) != state.prev_key {
            return false;
        }
        if messages.len() == state.prev_len {
            // 一条都没长 —— 只可能是重试：上一发报错了，messages 原样再投一次
            return state.observations.last().is_some_and(|o| o.error.is_some());
        }
        state
            .prev_reply
            .as_ref()
            .is_some_and(|prev| message_key(&messages[state.prev_len]) == *prev)
    }

    /// 开一条观测，顺便判定这次调用属于哪一轮。记账在调用**前**：模型报错时这条观测
    /// 也得留下，不然重试次数就数不出来了。
    fn open(&self, messages: &[Message]) -> usize {
        let mut state = self.state.lock().expect("ModelProbe 锁");
        let delta_from = if Self::is_continuation(&state, messages) {
            state.attempt += 1;
            state.prev_len
        } else {
            state.run += 1;
            state.attempt = 0;
            0
        };
        let gap_ms = state
            .last_return
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);
        let obs = Observation {
            index: state.observations.len(),
            run: state.run.max(0) as usize,
            attempt: state.attempt,
            n_messages: messages.len(),
            delta: messages[delta_from..].iter().map(row).collect(),
            gap_ms,
            ..Observation::default()
        };
        state.observations.push(obs);
        state.prev_len = messages.len();
        state.prev_key = prefix_key(messages, messages.len());
        state.observations.len() - 1
    }

    fn absorb(&self, idx: usize, turn: &ModelTurn) {
        let mut calls = Vec::new();
        for tc in turn.message.tool_calls.iter().flatten() {
            let spec = self.specs.get(&tc.name);
            let (schema_ok, schema_error) = match spec {
                None => (None, None),
                Some(spec) => {
                    match validate(&Value::Object(tc.arguments.clone()), &spec.parameters) {
                        Ok(_) => (Some(true), None),
                        Err(e) => (Some(false), Some(e.0)),
                    }
                }
            };
            calls.push(ToolCallObservation {
                call_id: tc.call_id.clone(),
                name: tc.name.clone(),
                arguments: tc.arguments.clone(),
                in_protocol: spec.is_some(),
                schema_ok,
                schema_error,
            });
        }
        let mut state = self.state.lock().expect("ModelProbe 锁");
        if let Some(obs) = state.observations.get_mut(idx) {
            obs.text = turn.message.content.clone();
            obs.finish_reason = turn.finish_reason.clone();
            obs.usage = kwargs! {
                "input_tokens" => json!(turn.usage.input_tokens),
                "output_tokens" => json!(turn.usage.output_tokens),
                "cached_tokens" => json!(turn.usage.cached_tokens),
            };
            obs.tool_calls = calls;
        }
    }
}

#[async_trait]
impl ModelPort for ModelProbe {
    fn name(&self) -> String {
        self.name.clone()
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        max_tokens: u32,
        temperature: f32,
    ) -> Result<ModelTurn, ModelError> {
        let obs_idx = self.open(messages);
        let call_idx = self.calls.record(
            "chat",
            kwargs! {
                "n_messages" => json!(messages.len()),
                "roles" => json!(messages.iter().map(|m| m.role.as_str()).collect::<Vec<_>>()),
                "tools" => json!(tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>()),
                "max_tokens" => json!(max_tokens),
                "temperature" => json!(temperature),
            },
        );

        let started = Instant::now();
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        let result = self
            .inner
            .chat(messages, tools, max_tokens, temperature)
            .await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        let elapsed_ms = started.elapsed().as_millis() as u64;

        match result {
            Err(err) => {
                let text = one_line(&err);
                {
                    let mut state = self.state.lock().expect("ModelProbe 锁");
                    if let Some(obs) = state.observations.get_mut(obs_idx) {
                        obs.elapsed_ms = elapsed_ms;
                        obs.error = Some(text.clone());
                    }
                    // 报错了就没有回牌被 append，重试认「一条没长」
                    state.prev_reply = None;
                    state.last_return = Some(Instant::now());
                }
                self.calls.set_error(call_idx, text);
                self.last_failed.store(true, Ordering::SeqCst);
                Err(err)
            }
            Ok(turn) => {
                {
                    let mut state = self.state.lock().expect("ModelProbe 锁");
                    if let Some(obs) = state.observations.get_mut(obs_idx) {
                        obs.elapsed_ms = elapsed_ms;
                    }
                    state.prev_reply = Some(message_key(&turn.message));
                }
                self.last_failed.store(false, Ordering::SeqCst);
                self.absorb(obs_idx, &turn);
                let names: Vec<String> = turn
                    .message
                    .tool_calls
                    .iter()
                    .flatten()
                    .map(|tc| tc.name.clone())
                    .collect();
                self.calls.set_result(
                    call_idx,
                    if names.is_empty() {
                        json!(format!("text:{}", clip(&turn.message.content, 30)))
                    } else {
                        json!(names)
                    },
                );
                self.state.lock().expect("ModelProbe 锁").last_return = Some(Instant::now());
                Ok(turn)
            }
        }
    }
}

fn one_line(err: &ModelError) -> String {
    let text = err.to_string();
    let head = text.lines().next().unwrap_or_default().trim().to_string();
    if head.is_empty() {
        "ModelError".to_string()
    } else {
        head
    }
}

// --------------------------------------------------------------------------
// 分析：把观测跟系统的反应对起来
// --------------------------------------------------------------------------

/// 一轮对话（≈ 一个 task）的观测集合。
struct Run {
    run: usize,
    obs: Vec<Observation>,
}

impl Run {
    /// 真出了牌的那几次（失败重试不算步）。
    fn steps(&self) -> Vec<&Observation> {
        self.obs.iter().filter(|o| o.ok()).collect()
    }
}

fn group(observations: &[Observation]) -> Vec<Run> {
    let mut by_run: BTreeMap<usize, Vec<Observation>> = BTreeMap::new();
    for o in observations {
        by_run.entry(o.run).or_default().push(o.clone());
    }
    by_run
        .into_iter()
        .map(|(run, obs)| Run { run, obs })
        .collect()
}

/// 第几步调了 final（0 起）。没调过返回 None。
fn final_index(run: &Run) -> Option<usize> {
    run.steps()
        .iter()
        .position(|o| o.tool_calls.iter().any(|tc| tc.name == "final"))
}

/// 「模型调用异常 / 5xx → 重试 2 次（2s / 5s）」的实际发生情况。
///
/// 一串连着报错的调用算一次 burst。**退避间隔记在「重试那一发」头上**：`gap_ms` 是
/// 「离上一次调用返回过了多久」，所以第 k 次失败之后的退避，体现在第 k+1 次调用的
/// gap 上。burst 后面还跟着一次成功调用 = worker 重试成功；后面什么都没有 = 重试用尽。
fn retry_bursts(run: &Run) -> Vec<Value> {
    let mut bursts = Vec::new();
    let obs = &run.obs;
    let mut i = 0;
    while i < obs.len() {
        if obs[i].ok() {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < obs.len() && !obs[j].ok() {
            j += 1;
        }
        // obs[i..j] 全是报错的；obs[i+1..=j] 是「每次失败之后 worker 又发的那一下」
        let followers = &obs[(i + 1).min(obs.len())..(j + 1).min(obs.len())];
        bursts.push(json!({
            "first_index": obs[i].index,
            "failures": j - i,
            "retried": followers.len(),
            "delays_ms": followers.iter().map(|o| o.gap_ms).collect::<Vec<_>>(),
            "errors": obs[i..j].iter().map(|o| o.error.clone()).collect::<Vec<_>>(),
            "recovered": j < obs.len(),
        }));
        i = j;
    }
    bursts
}

/// 区分纯文本兜底的两条分支。
///
/// * `steps==0` 且文本非空 → 视为 `final(reply=文本)`，这一轮到此为止。
/// * 其余（`steps>0`，或第一步就回了空文本）→ 回 system 提示并计 1 步。
///
/// 判据照抄 worker 主循环那一行（`step_index == 0 and text`）。**不能只看「下一步的
/// delta 里有没有 system」** —— 兜底之后 worker 立刻撞上 `max_steps`（或被取消）时压根
/// 没有下一次调用，那条 system 提示虽然进了 messages 却再也发不出去，光看 delta 会把
/// nudge 误报成 final。
fn fallback_text_only(run: &Run) -> (Vec<Value>, Vec<Value>) {
    let (mut as_final, mut nudged) = (Vec::new(), Vec::new());
    let steps = run.steps();
    for (i, o) in steps.iter().enumerate() {
        if !o.text_only() {
            continue;
        }
        let nudge = steps.get(i + 1).and_then(|nxt| {
            nxt.delta
                .iter()
                .rfind(|r| r.get("role") == Some(&json!("system")))
                .and_then(|r| r.get("content").cloned())
        });
        let row = json!({
            "run": run.run,
            "step": i,
            "text": clip(&o.text, MAX_TEXT_CHARS),
        });
        if i == 0 && !o.text.trim().is_empty() {
            as_final.push(row);
        } else {
            let mut row = row.as_object().cloned().unwrap_or_default();
            row.insert("nudge".to_string(), nudge.unwrap_or(Value::Null));
            nudged.push(Value::Object(row));
        }
    }
    (as_final, nudged)
}

/// 同一张牌连着出好几次 —— 真模型卡住时的典型姿态。
///
/// 实测 04_csv_to_chart：沙箱对每条不含 `savefig` 的代码都回 `exit_code=0` + 空 stdout，
/// 模型先合理地诊断了几步，然后对**逐字节相同**的 `run_python` 连发 31 次，一路烧到
/// `max_steps` 才停。两条计数兜底都接不住这种：`invalid_args` 要参数不合 schema，
/// `sandbox` 要工具报错，而这里工具**返回的是 ok=true**，只是内容为空。
///
/// 签名把一轮里的 tool_call 按顺序摊平算，只认**连续**相同：中间插进别的调用说明模型
/// 还在换招，不算卡住。
fn repeat_loops(run: &Run) -> Vec<Value> {
    let mut flat: Vec<(usize, &ToolCallObservation, String)> = Vec::new();
    for (i, o) in run.steps().iter().enumerate() {
        for tc in &o.tool_calls {
            flat.push((i, tc, fingerprint(&tc.name, &tc.arguments)));
        }
    }
    let mut loops = Vec::new();
    let mut i = 0;
    while i < flat.len() {
        let mut j = i + 1;
        while j < flat.len() && flat[j].2 == flat[i].2 {
            j += 1;
        }
        if j - i >= MIN_REPEAT_RUN {
            let (step, tc, _) = &flat[i];
            loops.push(json!({
                "run": run.run,
                "name": tc.name,
                "count": j - i,
                "first_step": step,
                "last_step": flat[j - 1].0,
                "arguments": tc.clipped_arguments(),
            }));
        }
        i = j;
    }
    loops
}

/// 从 evidence 链里把每次工具执行的判定捞出来（含 error code）。
///
/// 为什么不自己数：worker 对本地工具做的是就地校验，Gateway 做的才是
/// `ToolSpec.parameters` 全校验。系统究竟把哪一次判成了 `invalid_args`，
/// 只有 evidence 里的 `tool_result` 说了算。
fn tool_results(deps: &crate::deps::Deps) -> Vec<Value> {
    let mut rows = Vec::new();
    for (task_id, chain) in deps.evidence.chains() {
        for ev in chain {
            if ev.kind.as_str() != "tool_result" {
                continue;
            }
            let Some(payload) = &ev.payload else { continue };
            rows.push(json!({
                "task_id": task_id,
                "seq": ev.seq,
                "name": payload.get("name").cloned().unwrap_or(Value::Null),
                "ok": payload.get("ok").and_then(Value::as_bool).unwrap_or(false),
                "error": payload.get("error").cloned().unwrap_or(Value::Null),
            }));
        }
    }
    rows
}

fn counter_of(items: impl Iterator<Item = String>) -> Map<String, Value> {
    let mut out: BTreeMap<String, u64> = BTreeMap::new();
    for item in items {
        *out.entry(item).or_default() += 1;
    }
    out.into_iter().map(|(k, v)| (k, json!(v))).collect()
}

/// 「参数不合 schema → 回 invalid_args，连续 3 次 failed」的实际计数。
fn invalid_args(rows: &[Value]) -> Value {
    let (mut worst, mut streak) = (0usize, 0usize);
    let mut by_task: BTreeMap<String, u64> = BTreeMap::new();
    for r in rows {
        let is_invalid = r.get("error") == Some(&json!("invalid_args"));
        if is_invalid {
            streak += 1;
            let task = r["task_id"].as_str().unwrap_or_default().to_string();
            *by_task.entry(task).or_default() += 1;
        } else if r.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            streak = 0;
        }
        worst = worst.max(streak);
    }
    json!({
        "count": by_task.values().sum::<u64>(),
        "max_consecutive": worst,
        "by_tool": counter_of(rows.iter().filter(|r| r.get("error") == Some(&json!("invalid_args")))
            .map(|r| r["name"].as_str().unwrap_or_default().to_string())),
    })
}

fn tasks_of(deps: &crate::deps::Deps) -> Vec<Value> {
    deps.store
        .task_list()
        .iter()
        .map(|t| {
            json!({
                "task_no": t.task_no,
                "status": t.status.as_str(),
                "steps": t.steps,
                "max_steps": t.max_steps,
                "hit_max_steps": t.steps >= t.max_steps,
                "summary": clip(&t.result_summary, MAX_TEXT_CHARS),
            })
        })
        .collect()
}

/// 把一个场景跑完之后的观测汇成一份协议报告。
pub fn analyze(deps: &crate::deps::Deps) -> Map<String, Value> {
    let probe = &deps.model;
    let observations = probe.observations();
    let runs = group(&observations);

    let mut tool_names: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut violations = Vec::new();
    let mut as_final = Vec::new();
    let mut nudged = Vec::new();
    let mut retries = Vec::new();
    let mut loops = Vec::new();
    let mut run_rows = Vec::new();

    for run in &runs {
        let steps = run.steps();
        let final_at = final_index(run);
        for (i, o) in steps.iter().enumerate() {
            for tc in &o.tool_calls {
                tool_names.push(tc.name.clone());
                if !tc.in_protocol {
                    unknown.push(tc.name.clone());
                } else if tc.schema_ok == Some(false) {
                    violations.push(json!({
                        "run": run.run,
                        "step": i,
                        "name": tc.name,
                        "error": tc.schema_error,
                        "arguments": tc.clipped_arguments(),
                    }));
                }
            }
        }
        let (run_af, run_nudge) = fallback_text_only(run);
        as_final.extend(run_af);
        nudged.extend(run_nudge);
        loops.extend(repeat_loops(run));
        for burst in retry_bursts(run) {
            let mut row = burst.as_object().cloned().unwrap_or_default();
            row.insert("run".to_string(), json!(run.run));
            retries.push(Value::Object(row));
        }
        run_rows.push(json!({
            "run": run.run,
            "steps": steps.len(),
            "attempts": run.obs.len(),
            "reached_final": final_at.is_some(),
            "steps_to_final": final_at.map(|i| i + 1),
            "tools": steps.iter().flat_map(|o| o.tool_calls.iter().map(|tc| tc.name.clone())).collect::<Vec<_>>(),
            "steps_detail": run.obs.iter().map(Observation::to_json).collect::<Vec<_>>(),
        }));
    }

    let results = tool_results(deps);
    let tasks = tasks_of(deps);
    kwargs! {
        "model" => json!(probe.name),
        "chat_attempts" => json!(observations.len()),
        "chat_ok" => json!(observations.iter().filter(|o| o.ok()).count()),
        "runs" => Value::Array(run_rows.clone()),
        "reached_final" => json!(run_rows.iter().filter(|r| r["reached_final"] == json!(true)).count()),
        "tool_names" => Value::Object(counter_of(tool_names.into_iter())),
        "unknown_tools" => Value::Object(counter_of(unknown.into_iter())),
        "schema_violations" => Value::Array(violations),
        // 故意不放进 fallbacks：没有哪一条兜底接得住原地打转，它是「缺兜底」的证据
        "repeat_loops" => Value::Array(loops),
        "fallbacks" => json!({
            "text_only_as_final": as_final,
            "text_only_nudge": nudged,
            "invalid_args": invalid_args(&results),
            "model_retry": retries,
            "not_found": counter_of(results.iter().filter(|r| r.get("error") == Some(&json!("not_found")))
                .map(|r| r["name"].as_str().unwrap_or_default().to_string())),
            "sandbox_errors": results.iter().filter(|r| r.get("error") == Some(&json!("sandbox"))).count(),
        }),
        "tasks" => Value::Array(tasks.clone()),
        "hit_max_steps" => json!(tasks.iter().filter(|t| t["hit_max_steps"] == json!(true))
            .map(|t| t["task_no"].clone()).collect::<Vec<_>>()),
        "outbound_texts" => json!(deps.platform.texts().iter().map(|t| clip(t, MAX_TEXT_CHARS)).collect::<Vec<_>>()),
    }
}

// --------------------------------------------------------------------------
// 人话摘要
// --------------------------------------------------------------------------

fn verdict(report: &Map<String, Value>) -> String {
    let runs = report
        .get("runs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if runs.is_empty() {
        return "模型一次都没被调用".to_string();
    }
    let reached = report
        .get("reached_final")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let steps: Vec<u64> = runs
        .iter()
        .filter_map(|r| r.get("steps_to_final").and_then(Value::as_u64))
        .collect();
    let tail = if steps.is_empty() {
        String::new()
    } else {
        format!("，到 final 用了 {steps:?} 步")
    };
    format!("{} 轮 / final {reached} 轮{tail}", runs.len())
}

fn step_line(o: &Value) -> String {
    if let Some(err) = o.get("error").and_then(Value::as_str) {
        return format!(
            "#{} ✗ {err}（gap {}ms）",
            o["index"],
            o.get("gap_ms").and_then(Value::as_u64).unwrap_or(0)
        );
    }
    let calls = o
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if calls.is_empty() {
        return format!(
            "#{} 纯文本 {:?}（finish={}）",
            o["index"],
            o.get("text").and_then(Value::as_str).unwrap_or_default(),
            o.get("finish_reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
        );
    }
    let parts: Vec<String> = calls
        .iter()
        .map(|tc| {
            let mut mark = String::new();
            if tc.get("schema_ok") == Some(&json!(false)) {
                mark.push_str(" ✗schema");
            }
            if tc.get("in_protocol") != Some(&json!(true)) {
                mark.push_str(" ✗协议外");
            }
            let args: Vec<&str> = tc
                .get("arguments")
                .and_then(Value::as_object)
                .map(|m| m.keys().map(String::as_str).collect())
                .unwrap_or_default();
            format!(
                "{}({}){mark}",
                tc["name"].as_str().unwrap_or_default(),
                args.join(", ")
            )
        })
        .collect();
    format!("#{} {}", o["index"], parts.join(" "))
}

/// 把若干场景的报告渲成一段能读的文字。写 stderr，不污染 stdout 的 JSON 摘要。
pub fn render_digest(rows: &[(String, Map<String, Value>)]) -> String {
    let mut out = vec!["=== 协议出牌报告（模型实测）===".to_string()];
    for (name, report) in rows {
        if report.is_empty() {
            out.push(format!("\n--- {name} ---\n  （没装探针）"));
            continue;
        }
        let empty = Map::new();
        let fb = report
            .get("fallbacks")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        out.push(format!(
            "\n--- {name} · {} · {} ---",
            report
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            verdict(report)
        ));
        for run in report
            .get("runs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let head = format!(
                "  轮 {}：{} 步 / {} 次调用 · {}",
                run["run"],
                run["steps"],
                run["attempts"],
                if run["reached_final"] == json!(true) {
                    format!("final@{}", run["steps_to_final"])
                } else {
                    "未 final".to_string()
                }
            );
            out.push(head);
            for o in run
                .get("steps_detail")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                out.push(format!("    {}", step_line(o)));
            }
        }
        if let Some(unknown) = report.get("unknown_tools").and_then(Value::as_object)
            && !unknown.is_empty()
        {
            out.push(format!(
                "  协议外工具名：{}",
                Value::Object(unknown.clone())
            ));
        }
        for v in report
            .get("schema_violations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            out.push(format!(
                "  参数不合 schema：轮{} 步{} {} -> {}",
                v["run"], v["step"], v["name"], v["error"]
            ));
        }
        for hit in fb
            .get("text_only_as_final")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            out.push(format!(
                "  兜底[纯文本→final]：轮{} 步{} {}",
                hit["run"], hit["step"], hit["text"]
            ));
        }
        for hit in fb
            .get("text_only_nudge")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let tail = if hit.get("nudge").map(Value::is_null).unwrap_or(true) {
                "（提示没能再投出去：兜底之后就没有下一步了）".to_string()
            } else {
                format!("{}", hit["nudge"])
            };
            out.push(format!(
                "  兜底[纯文本→system 提示]：轮{} 步{} {tail}",
                hit["run"], hit["step"]
            ));
        }
        for lp in report
            .get("repeat_loops")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let args: Vec<&str> = lp
                .get("arguments")
                .and_then(Value::as_object)
                .map(|m| m.keys().map(String::as_str).collect())
                .unwrap_or_default();
            out.push(format!(
                "  ⚠ 原地打转：轮{} 步{}–{} 连着 {} 次一模一样的 {}({}) —— 没有哪条兜底接得住，只有 max_steps",
                lp["run"], lp["first_step"], lp["last_step"], lp["count"], lp["name"], args.join(", ")
            ));
        }
        if let Some(ia) = fb.get("invalid_args")
            && ia.get("count").and_then(Value::as_u64).unwrap_or(0) > 0
        {
            out.push(format!(
                "  兜底[invalid_args]：{} 次，最长连续 {} 次（上限 3 触发 failed）{}",
                ia["count"], ia["max_consecutive"], ia["by_tool"]
            ));
        }
        for burst in fb
            .get("model_retry")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            out.push(format!(
                "  兜底[模型异常重试]：轮{} 报了 {} 次 / 又重试 {} 次，退避 {}ms，{}；错误：{}",
                burst["run"],
                burst["failures"],
                burst["retried"],
                burst["delays_ms"],
                if burst["recovered"] == json!(true) {
                    "最终成功"
                } else {
                    "重试用尽"
                },
                burst["errors"]
            ));
        }
        if let Some(nf) = fb.get("not_found").and_then(Value::as_object)
            && !nf.is_empty()
        {
            out.push(format!(
                "  工具名查无此人（not_found）：{}",
                Value::Object(nf.clone())
            ));
        }
        if fb
            .get("sandbox_errors")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
        {
            out.push(format!(
                "  沙箱错误：{} 次（连续 2 次 failed）",
                fb["sandbox_errors"]
            ));
        }
        if report
            .get("hit_max_steps")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
        {
            // 带上终态：`hit_max_steps` 只是「步数用满了」，跟「因为撞上限而失败」不是一回事
            let hits: Map<String, Value> = report
                .get("tasks")
                .and_then(Value::as_array)
                .map(|ts| {
                    ts.iter()
                        .filter(|t| t["hit_max_steps"] == json!(true))
                        .map(|t| {
                            (
                                t["task_no"].as_str().unwrap_or_default().to_string(),
                                t["status"].clone(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            out.push(format!(
                "  步数用满 max_steps 的任务：{}",
                Value::Object(hits)
            ));
        }
        let terminal: Vec<String> = report
            .get("tasks")
            .and_then(Value::as_array)
            .map(|ts| {
                ts.iter()
                    .map(|t| format!("({}, {})", t["task_no"], t["status"]))
                    .collect()
            })
            .unwrap_or_default();
        out.push(format!(
            "  任务终态：{}",
            if terminal.is_empty() {
                "(没建任务)".to_string()
            } else {
                format!("[{}]", terminal.join(", "))
            }
        ));
    }
    out.join("\n")
}
