//! FakeModel —— `ModelPort` 的脚本化替身（对应旧 `aite/testing/fake_model.py`）。
//!
//! 一条脚本一步出牌，worker 每调一次 `chat` 消费一步。三种出牌都能造：
//!
//! * `tool_calls`：一步里可以有多个 tool_call，worker 按顺序处理
//! * `text`（无 tool_call）：那条兜底 —— `steps==0` 时视为 `final(reply=文本)`，
//!   `steps>0` 时回 system 提示并计 1 步。**这条逻辑归 worker（R5）**，替身只负责把这种牌造出来
//! * `error`：模型调用直接报错，演「模型调用异常 / 5xx」
//!
//! 两个「不往前走」的旋钮是正交的，别混：
//!
//! * `repeat: inf` 是**无限出牌**：这一步反复出，脚本到此为止（08_step_limit 用它把模型钉在
//!   checklist_note 上，好撞到 max_steps）。
//! * `hold_ticks` 是**这一次不返回**：`chat()` 先把控制权让回运行时若干次再出牌
//!   （07_commands 用它让任务在 `!status` / `!stop` 到达时还活着）。让出的是 tokio 的调度
//!   tick（`yield_now`），不是墙钟 —— 不 sleep 真实时间，跑多少次结果都一样。
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use aite_contracts::{
    Message, ModelError, ModelPort, ModelTurn, Role, ToolCallRequest, ToolSpec, Usage,
};
use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use crate::kwargs;
use crate::recorder::CallLog;

/// 脚本里 `error:` 那一步报的错，用来演模型侧异常。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FakeModelError(pub String);

/// 脚本用完了还被继续调用 —— 场景写漏了，报告里要能一眼看出是这个原因。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ScriptExhausted(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ScriptedToolCall {
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Map<String, Value>,
    #[serde(default)]
    pub call_id: Option<String>,
}

/// 这一步重复几次；`Inf` = 永远重复（脚本到此为止，后面的步骤不会再被消费）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Repeat {
    Times(u32),
    #[serde(serialize_with = "ser_inf")]
    Inf,
}

fn ser_inf<S: serde::Serializer>(s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str("inf")
}

impl Default for Repeat {
    fn default() -> Self {
        Repeat::Times(1)
    }
}

impl<'de> Deserialize<'de> for Repeat {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Value::deserialize(d)?;
        match &raw {
            Value::String(s) if s == "inf" => Ok(Repeat::Inf),
            Value::Number(n) => match n.as_i64() {
                Some(v) if v >= 1 => Ok(Repeat::Times(v as u32)),
                _ => Err(serde::de::Error::custom(format!(
                    "repeat 必须是 >=1 的整数或 'inf'，收到 {raw}"
                ))),
            },
            _ => Err(serde::de::Error::custom(format!(
                "repeat 必须是 >=1 的整数或 'inf'，收到 {raw}"
            ))),
        }
    }
}

fn de_hold_ticks<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let raw = i64::deserialize(d)?;
    if raw < 0 {
        return Err(serde::de::Error::custom(format!(
            "hold_ticks 必须是 >=0 的整数，收到 {raw}"
        )));
    }
    Ok(raw as u32)
}

/// 一步出牌。`tool_calls` 与 `text` 可以同时有（模型边说话边调工具）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ScriptStep {
    pub text: String,
    pub tool_calls: Vec<ScriptedToolCall>,
    pub usage: Usage,
    pub finish_reason: Option<String>,
    pub repeat: Repeat,
    /// 非空则这一步报 FakeModelError(error)
    pub error: Option<String>,
    /// 出牌前先把控制权让回运行时几次（`yield_now()` x N）。0 = 不让，立刻出牌。
    ///
    /// 为什么要有它：替身全是瞬时返回的，一次 `chat()` 里没有任何真会挂起的 await 点，
    /// worker 被调度上之后就会一口气把脚本跑到头 —— 投递那一头根本插不进来。凡是「任务
    /// 还活着的时候才有意义」的场景（`!status` / `!stop`）就此没得可测。给某一步加上
    /// hold_ticks，worker 就确定性地停在「下一次 chat」上，上一步的副作用（卡片、沙箱）
    /// 都已经落地，而别的任务终于有机会跑。
    ///
    /// 让满 N 次就照常出牌（别的地方也可以调 `release_holds()` 提前放行），所以哪怕等的
    /// 那件事永远不发生，场景也只会以断言失败收场，不会挂死。
    #[serde(deserialize_with = "de_hold_ticks")]
    pub hold_ticks: u32,
}

impl ScriptStep {
    pub fn from_value(v: Value) -> Result<Self, String> {
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    /// 一步 `final(reply=…)`，测试里最常用。
    pub fn final_reply(reply: &str) -> Self {
        Self {
            tool_calls: vec![ScriptedToolCall {
                name: "final".to_string(),
                arguments: crate::kwargs! {"reply" => json!(reply)},
                call_id: None,
            }],
            ..Self::default()
        }
    }

    /// 一步纯文本。
    pub fn text(text: &str) -> Self {
        Self {
            text: text.to_string(),
            ..Self::default()
        }
    }

    /// 一步单工具调用。
    pub fn tool(name: &str, arguments: serde_json::Map<String, Value>) -> Self {
        Self {
            tool_calls: vec![ScriptedToolCall {
                name: name.to_string(),
                arguments,
                call_id: None,
            }],
            ..Self::default()
        }
    }
}

#[derive(Default)]
struct Cursor {
    /// 指向下一个「还没用尽」的步骤
    index: usize,
    /// 当前步骤已经出过几次牌
    served_here: u32,
}

/// `ModelPort` 的替身。`name` 与契约的 `ModelPort::name` 对齐。
pub struct FakeModel {
    pub name: String,
    pub script: Vec<ScriptStep>,
    pub calls: CallLog,
    cursor: Mutex<Cursor>,
    turns_served: AtomicUsize,
    holds: AtomicUsize,
    hold_ticks_yielded: AtomicUsize,
    holds_released: AtomicBool,
}

impl FakeModel {
    pub fn new(script: Vec<ScriptStep>) -> Self {
        Self {
            name: "scripted".to_string(),
            script,
            calls: CallLog::new(),
            cursor: Mutex::new(Cursor::default()),
            turns_served: AtomicUsize::new(0),
            holds: AtomicUsize::new(0),
            hold_ticks_yielded: AtomicUsize::new(0),
            holds_released: AtomicBool::new(false),
        }
    }

    /// 从 JSON / YAML 值造脚本（场景 yaml 的 `model_script:` 走这条）。
    pub fn from_values(values: Vec<Value>) -> Result<Self, String> {
        let mut script = Vec::with_capacity(values.len());
        for v in values {
            script.push(ScriptStep::from_value(v)?);
        }
        Ok(Self::new(script))
    }

    pub fn named(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    /// 让正在 hold 的那一步立刻出牌，之后的 hold 步也不再挂。
    ///
    /// 07_commands 用不着它（那里靠 hold_ticks 的上限自己放行）。这条路留给
    /// 「要等的事已经发生了，别再空转」的调用方。
    pub fn release_holds(&self) {
        self.holds_released.store(true, Ordering::SeqCst);
    }

    pub fn turns_served(&self) -> usize {
        self.turns_served.load(Ordering::SeqCst)
    }

    pub fn holds(&self) -> usize {
        self.holds.load(Ordering::SeqCst)
    }

    pub fn hold_ticks_yielded(&self) -> usize {
        self.hold_ticks_yielded.load(Ordering::SeqCst)
    }

    pub fn holds_released(&self) -> bool {
        self.holds_released.load(Ordering::SeqCst)
    }

    pub fn call_count(&self) -> usize {
        self.calls.count("chat")
    }

    /// 模型出过的所有 tool_call 名字（含本地 checklist_* 与 final）。
    pub fn tool_names_emitted(&self) -> Vec<String> {
        self.calls
            .of("chat")
            .iter()
            .filter_map(|c| c.result.as_ref())
            .filter_map(|v| v.as_array())
            .flat_map(|names| names.iter().filter_map(|n| n.as_str().map(String::from)))
            .collect()
    }

    // ---- 内部 -----------------------------------------------------------

    async fn hold(&self, ticks: u32) {
        self.holds.fetch_add(1, Ordering::SeqCst);
        for _ in 0..ticks {
            if self.holds_released.load(Ordering::SeqCst) {
                return;
            }
            tokio::task::yield_now().await;
            self.hold_ticks_yielded.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// 取下一步，并返回它在脚本里的下标（进 `raw.scripted_step`）。
    fn take(&self) -> Result<(ScriptStep, usize), ScriptExhausted> {
        let mut cursor = self.cursor.lock().expect("FakeModel 锁");
        if cursor.index >= self.script.len() {
            return Err(ScriptExhausted(format!(
                "模型脚本已用尽：共 {} 步，这是第 {} 次调用。\
                 给 model_script 补步骤，或给最后一步写 repeat: inf",
                self.script.len(),
                self.turns_served() + 1
            )));
        }
        let index = cursor.index;
        let step = self.script[index].clone();
        cursor.served_here += 1;
        if let Repeat::Times(n) = step.repeat
            && cursor.served_here >= n
        {
            cursor.index += 1;
            cursor.served_here = 0;
        }
        Ok((step, index))
    }
}

impl std::fmt::Debug for FakeModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "<FakeModel {} {}/{} steps served>",
            self.name,
            self.turns_served(),
            self.script.len()
        )
    }
}

#[async_trait]
impl ModelPort for FakeModel {
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
        let idx = self.calls.record(
            "chat",
            kwargs! {
                "n_messages" => json!(messages.len()),
                "roles" => json!(messages.iter().map(|m| m.role.as_str()).collect::<Vec<_>>()),
                "tools" => json!(tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>()),
                "max_tokens" => json!(max_tokens),
                "temperature" => json!(temperature),
            },
        );

        let (step, cursor_index) = match self.take() {
            Ok(v) => v,
            Err(exhausted) => {
                self.calls.set_error(idx, exhausted.0.clone());
                return Err(ModelError::Upstream(exhausted.0));
            }
        };

        if step.hold_ticks > 0 {
            self.hold(step.hold_ticks).await;
        }
        if let Some(error) = &step.error {
            self.calls.set_error(idx, error.clone());
            return Err(ModelError::Upstream(error.clone()));
        }

        let served = self.turns_served();
        let tool_calls: Vec<ToolCallRequest> = step
            .tool_calls
            .iter()
            .enumerate()
            .map(|(j, tc)| ToolCallRequest {
                call_id: tc
                    .call_id
                    .clone()
                    .unwrap_or_else(|| format!("call_{served}_{j}")),
                name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            })
            .collect();

        let finish_reason = step.finish_reason.clone().unwrap_or_else(|| {
            if tool_calls.is_empty() {
                "stop".to_string()
            } else {
                "tool_calls".to_string()
            }
        });
        let turn = ModelTurn {
            message: Message {
                role: Role::Assistant,
                content: step.text.clone(),
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls.clone())
                },
                tool_call_id: None,
                name: None,
            },
            usage: step.usage,
            finish_reason,
            raw: kwargs! {
                "scripted_step" => json!(cursor_index),
                "served" => json!(served),
            },
        };
        self.turns_served.fetch_add(1, Ordering::SeqCst);
        self.calls.set_result(
            idx,
            if tool_calls.is_empty() {
                json!(format!(
                    "text:{}",
                    step.text.chars().take(30).collect::<String>()
                ))
            } else {
                json!(
                    tool_calls
                        .iter()
                        .map(|tc| tc.name.clone())
                        .collect::<Vec<_>>()
                )
            },
        );
        Ok(turn)
    }
}
