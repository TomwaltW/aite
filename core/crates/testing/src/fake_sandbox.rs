//! FakeSandbox —— `SandboxPort` 的替身（对应旧 `aite/testing/fake_sandbox.py`）：
//! 内存文件系统 + 脚本化 exec。
//!
//! `exec` 不真跑 Python：场景在 `exec_script` 里声明「代码里出现某个子串时，返回什么
//! exit_code / stdout，并往 /work 下写哪些文件」。04_csv_to_chart 靠 `writes` 造出
//! `/work/out.png`，再由 `final(artifacts)` 走 `get_file` → `send_file`。
//!
//! 时钟可注入，`reap_idle` 因此能在测试里被确定性地触发，不用真等 5 分钟。
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aite_contracts::{
    ExecRequest, ExecResult, FileEntry, SandboxError, SandboxErrorKind, SandboxPort, SandboxSpec,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::kwargs;
use crate::recorder::CallLog;
use crate::samples::BUILTINS;

/// 沙箱替身自己判定的错（未知 sandbox_id、路径越界等）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FakeSandboxError(pub String);

impl From<FakeSandboxError> for SandboxError {
    fn from(e: FakeSandboxError) -> Self {
        SandboxError::new(SandboxErrorKind::Internal, e.0)
    }
}

fn not_found(message: String) -> SandboxError {
    SandboxError::new(SandboxErrorKind::FileNotFound, message)
}

/// 场景 yaml 里的文件内容：str / `builtin:png` / `{b64: …}` / `{builtin: …}`。
pub fn as_bytes(value: &Value) -> Result<Vec<u8>, FakeSandboxError> {
    match value {
        Value::Object(map) => {
            if let Some(b64) = map.get("b64").and_then(Value::as_str) {
                return base64_decode(b64);
            }
            if let Some(key) = map.get("builtin").and_then(Value::as_str) {
                return builtin(key);
            }
            Err(FakeSandboxError(format!("看不懂的文件内容声明：{value}")))
        }
        Value::String(s) => match s.strip_prefix("builtin:") {
            Some(key) => builtin(key),
            None => Ok(s.as_bytes().to_vec()),
        },
        other => Err(FakeSandboxError(format!("看不懂的文件内容声明：{other}"))),
    }
}

fn builtin(key: &str) -> Result<Vec<u8>, FakeSandboxError> {
    BUILTINS.get(key).cloned().ok_or_else(|| {
        let available: Vec<&&str> = BUILTINS.keys().collect();
        FakeSandboxError(format!("没有内置样本 {key:?}，可用：{available:?}"))
    })
}

/// 标准 base64 解码（没有 base64 crate，加依赖要停下报告）。
fn base64_decode(text: &str) -> Result<Vec<u8>, FakeSandboxError> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lut = [255u8; 256];
    for (i, c) in ALPHABET.iter().enumerate() {
        lut[*c as usize] = i as u8;
    }
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for ch in text.bytes() {
        if ch == b'=' || ch.is_ascii_whitespace() {
            continue;
        }
        let v = lut[ch as usize];
        if v == 255 {
            return Err(FakeSandboxError(format!(
                "b64 里有非法字符：{:?}",
                ch as char
            )));
        }
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

/// 一条 exec 脚本。`match` 为 None 时匹配任何代码。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecScriptStep {
    #[serde(rename = "match")]
    pub match_: Option<String>,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub truncated: bool,
    /// 沙箱内路径 -> 内容；执行「成功」时写进内存 FS
    pub writes: BTreeMap<String, Value>,
    /// 报错而不是返回结果，演「沙箱创建/执行失败」
    pub error: Option<String>,
    /// 可复用几次；None = 无限
    pub times: Option<u32>,
}

impl Default for ExecScriptStep {
    fn default() -> Self {
        Self {
            match_: None,
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 5,
            truncated: false,
            writes: BTreeMap::new(),
            error: None,
            times: None,
        }
    }
}

#[derive(Debug, Clone)]
struct Box_ {
    task_id: String,
    #[allow(dead_code)]
    spec: SandboxSpec,
    files: BTreeMap<String, Vec<u8>>,
    last_active: f64,
    released: bool,
}

#[derive(Default)]
struct State {
    boxes: BTreeMap<String, Box_>,
    order: Vec<String>,
    released_ids: Vec<String>,
    used: BTreeMap<usize, u32>,
    next_id: u64,
}

type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;

/// `SandboxPort` 的替身。
pub struct FakeSandbox {
    pub calls: CallLog,
    pub exec_script: Vec<ExecScriptStep>,
    clock: Clock,
    state: Mutex<State>,
}

impl Default for FakeSandbox {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

fn monotonic() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}

impl FakeSandbox {
    pub fn new(exec_script: Vec<ExecScriptStep>) -> Self {
        Self {
            calls: CallLog::new(),
            exec_script,
            clock: Arc::new(monotonic),
            state: Mutex::new(State {
                next_id: 1,
                ..State::default()
            }),
        }
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    pub fn from_values(values: &[Value]) -> Result<Self, String> {
        let mut script = Vec::with_capacity(values.len());
        for v in values {
            script.push(serde_json::from_value(v.clone()).map_err(|e| e.to_string())?);
        }
        Ok(Self::new(script))
    }

    // ---- 给断言用 --------------------------------------------------------

    pub fn alive(&self) -> Vec<String> {
        let state = self.state.lock().expect("FakeSandbox 锁");
        state
            .boxes
            .iter()
            .filter(|(_, b)| !b.released)
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn all_files(&self) -> BTreeMap<String, BTreeMap<String, usize>> {
        let state = self.state.lock().expect("FakeSandbox 锁");
        state
            .boxes
            .iter()
            .map(|(id, b)| {
                (
                    id.clone(),
                    b.files.iter().map(|(p, d)| (p.clone(), d.len())).collect(),
                )
            })
            .collect()
    }

    pub fn released_ids(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("FakeSandbox 锁")
            .released_ids
            .clone()
    }

    pub fn box_ids(&self) -> Vec<String> {
        self.state.lock().expect("FakeSandbox 锁").order.clone()
    }

    pub fn task_of(&self, sandbox_id: &str) -> Option<String> {
        self.state
            .lock()
            .expect("FakeSandbox 锁")
            .boxes
            .get(sandbox_id)
            .map(|b| b.task_id.clone())
    }

    // ---- 内部 -----------------------------------------------------------

    fn require_alive(&self, state: &State, sandbox_id: &str) -> Result<(), SandboxError> {
        match state.boxes.get(sandbox_id) {
            None => {
                let known: Vec<&String> = state.boxes.keys().collect();
                Err(SandboxError::new(
                    SandboxErrorKind::NotFound,
                    format!("未知 sandbox_id={sandbox_id:?}（在册：{known:?}）"),
                ))
            }
            Some(b) if b.released => Err(SandboxError::new(
                SandboxErrorKind::NotFound,
                format!("沙箱 {sandbox_id} 已经 release 过了"),
            )),
            Some(_) => Ok(()),
        }
    }

    fn check_path(path: &str) -> Result<(), SandboxError> {
        if !path.starts_with("/work") {
            return Err(FakeSandboxError(format!(
                "路径必须在 /work 下（SandboxPort.put_file）：{path:?}"
            ))
            .into());
        }
        Ok(())
    }

    /// 顺序扫：`match` 为 None 匹配任何；否则子串命中；`times` 用完就跳过。
    fn match_step(&self, code: &str) -> Option<ExecScriptStep> {
        let mut state = self.state.lock().expect("FakeSandbox 锁");
        for (i, step) in self.exec_script.iter().enumerate() {
            if let Some(needle) = &step.match_
                && !code.contains(needle.as_str())
            {
                continue;
            }
            let used = state.used.get(&i).copied().unwrap_or(0);
            if let Some(times) = step.times
                && used >= times
            {
                continue;
            }
            state.used.insert(i, used + 1);
            return Some(step.clone());
        }
        None
    }
}

#[async_trait]
impl SandboxPort for FakeSandbox {
    async fn acquire(&self, task_id: &str, spec: &SandboxSpec) -> Result<String, SandboxError> {
        let sandbox_id = {
            let mut state = self.state.lock().expect("FakeSandbox 锁");
            let n = state.next_id;
            state.next_id += 1;
            format!("sb-{n}")
        };
        self.calls.record(
            "acquire",
            kwargs! {
                "task_id" => json!(task_id),
                "image" => json!(spec.image),
                "sandbox_id" => json!(sandbox_id),
            },
        );
        let now = (self.clock)();
        let mut state = self.state.lock().expect("FakeSandbox 锁");
        state.boxes.insert(
            sandbox_id.clone(),
            Box_ {
                task_id: task_id.to_string(),
                spec: spec.clone(),
                files: BTreeMap::new(),
                last_active: now,
                released: false,
            },
        );
        state.order.push(sandbox_id.clone());
        Ok(sandbox_id)
    }

    async fn exec(&self, sandbox_id: &str, req: &ExecRequest) -> Result<ExecResult, SandboxError> {
        let idx = self.calls.record(
            "exec",
            kwargs! {"sandbox_id" => json!(sandbox_id), "timeout_sec" => json!(req.timeout_sec)},
        );
        let now = (self.clock)();
        {
            let mut state = self.state.lock().expect("FakeSandbox 锁");
            self.require_alive(&state, sandbox_id)?;
            if let Some(b) = state.boxes.get_mut(sandbox_id) {
                b.last_active = now;
            }
        }

        let Some(step) = self.match_step(&req.code) else {
            // 一条都不匹配 → 无害默认值（清单 §11 第 22 条）
            self.calls.set_result(idx, json!("default"));
            return Ok(ExecResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: 1,
                truncated: false,
                files_out: Vec::new(),
            });
        };

        if let Some(error) = &step.error {
            self.calls.set_error(idx, error.clone());
            return Err(FakeSandboxError(error.clone()).into());
        }

        let mut files_out = Vec::new();
        for (path, value) in &step.writes {
            let data = as_bytes(value).map_err(SandboxError::from)?;
            Self::check_path(path)?;
            let size = data.len();
            self.state
                .lock()
                .expect("FakeSandbox 锁")
                .boxes
                .get_mut(sandbox_id)
                .expect("刚校验过还在")
                .files
                .insert(path.clone(), data);
            files_out.push(FileEntry {
                path: path.clone(),
                size: size as i64,
            });
        }
        self.calls.set_result(
            idx,
            json!(format!(
                "exit={} files={:?}",
                step.exit_code,
                files_out.iter().map(|f| f.path.clone()).collect::<Vec<_>>()
            )),
        );
        Ok(ExecResult {
            exit_code: step.exit_code,
            stdout: step.stdout.clone(),
            stderr: step.stderr.clone(),
            duration_ms: step.duration_ms,
            truncated: step.truncated,
            files_out,
        })
    }

    async fn put_file(
        &self,
        sandbox_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), SandboxError> {
        self.calls.record(
            "put_file",
            kwargs! {
                "sandbox_id" => json!(sandbox_id),
                "path" => json!(path),
                "size" => json!(data.len()),
            },
        );
        Self::check_path(path)?;
        let now = (self.clock)();
        let mut state = self.state.lock().expect("FakeSandbox 锁");
        self.require_alive(&state, sandbox_id)?;
        let b = state.boxes.get_mut(sandbox_id).expect("刚校验过还在");
        b.files.insert(path.to_string(), data.to_vec());
        b.last_active = now;
        Ok(())
    }

    async fn get_file(&self, sandbox_id: &str, path: &str) -> Result<Vec<u8>, SandboxError> {
        let idx = self.calls.record(
            "get_file",
            kwargs! {"sandbox_id" => json!(sandbox_id), "path" => json!(path)},
        );
        let now = (self.clock)();
        let mut state = self.state.lock().expect("FakeSandbox 锁");
        self.require_alive(&state, sandbox_id)?;
        let b = state.boxes.get_mut(sandbox_id).expect("刚校验过还在");
        match b.files.get(path).cloned() {
            Some(data) => {
                b.last_active = now;
                Ok(data)
            }
            None => {
                let have: Vec<&String> = b.files.keys().collect();
                let message = format!("沙箱 {sandbox_id} 里没有 {path}（有的是：{have:?}）");
                drop(state);
                self.calls.set_error(idx, "not found");
                Err(not_found(message))
            }
        }
    }

    async fn list_files(&self, sandbox_id: &str) -> Result<Vec<String>, SandboxError> {
        self.calls
            .record("list_files", kwargs! {"sandbox_id" => json!(sandbox_id)});
        let state = self.state.lock().expect("FakeSandbox 锁");
        self.require_alive(&state, sandbox_id)?;
        Ok(state.boxes[sandbox_id].files.keys().cloned().collect())
    }

    async fn touch(&self, sandbox_id: &str) -> Result<(), SandboxError> {
        self.calls
            .record("touch", kwargs! {"sandbox_id" => json!(sandbox_id)});
        let now = (self.clock)();
        let mut state = self.state.lock().expect("FakeSandbox 锁");
        self.require_alive(&state, sandbox_id)?;
        state
            .boxes
            .get_mut(sandbox_id)
            .expect("刚校验过还在")
            .last_active = now;
        Ok(())
    }

    async fn release(&self, sandbox_id: &str) -> Result<(), SandboxError> {
        self.calls
            .record("release", kwargs! {"sandbox_id" => json!(sandbox_id)});
        let mut state = self.state.lock().expect("FakeSandbox 锁");
        match state.boxes.get_mut(sandbox_id) {
            None => Ok(()), // 契约要求幂等：未知 id 静默 no-op
            Some(b) if b.released => Ok(()),
            Some(b) => {
                b.released = true;
                state.released_ids.push(sandbox_id.to_string());
                Ok(())
            }
        }
    }

    async fn reap_idle(&self, idle_sec: u32) -> Result<Vec<String>, SandboxError> {
        self.calls
            .record("reap_idle", kwargs! {"idle_sec" => json!(idle_sec)});
        let now = (self.clock)();
        let candidates: Vec<String> = {
            let state = self.state.lock().expect("FakeSandbox 锁");
            state
                .order
                .iter()
                .filter(|id| {
                    state
                        .boxes
                        .get(*id)
                        .is_some_and(|b| !b.released && now - b.last_active >= f64::from(idle_sec))
                })
                .cloned()
                .collect()
        };
        for id in &candidates {
            self.release(id).await?;
        }
        Ok(candidates)
    }
}
