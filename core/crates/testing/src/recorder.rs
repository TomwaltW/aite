//! 调用记账（对应旧 `aite/testing/recorder.py`）。
//!
//! 替身自己不做断言，只把「谁在什么时候被以什么参数调用了」记下来；断言留给
//! `aite-evals` 的 expect DSL 与各 crate 的测试。这样同一份替身既能给单测用，
//! 也能给场景 runner 用，不必为每种断言各写一个替身。
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value};

/// 模块级全局计数器：seq 跨替身单调，能排出不同替身之间的先后（清单 §11 第 24 条）。
static SEQ: AtomicU64 = AtomicU64::new(0);

fn next_seq() -> u64 {
    SEQ.fetch_add(1, Ordering::SeqCst)
}

/// 一次被调用的记录。`error` 非空表示这次调用以异常收场。
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub method: String,
    pub kwargs: Map<String, Value>,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub seq: u64,
}

impl Call {
    /// 取一个 kwarg（断言里常用）。
    pub fn arg(&self, key: &str) -> Option<&Value> {
        self.kwargs.get(key)
    }

    /// 取一个字符串 kwarg。
    pub fn arg_str(&self, key: &str) -> Option<&str> {
        self.kwargs.get(key).and_then(Value::as_str)
    }
}

impl std::fmt::Display for Call {
    /// 失败信息里要能一眼看出调了什么（对应 Python 的 `__repr__`）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let args: Vec<String> = self
            .kwargs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        write!(f, "<Call#{} {}({})", self.seq, self.method, args.join(", "))?;
        if let Some(err) = &self.error {
            write!(f, " !{err}")?;
        }
        f.write_str(">")
    }
}

/// 按调用顺序保存 Call；seq 全局单调，跨替身也能排出先后。
#[derive(Debug, Default)]
pub struct CallLog {
    calls: Mutex<Vec<Call>>,
}

impl CallLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一笔，返回它在本 log 里的下标（回填 result / error 用）。
    pub fn record(&self, method: &str, kwargs: Map<String, Value>) -> usize {
        let mut guard = self.calls.lock().expect("CallLog 锁");
        guard.push(Call {
            method: method.to_string(),
            kwargs,
            result: None,
            error: None,
            seq: next_seq(),
        });
        guard.len() - 1
    }

    pub fn set_result(&self, index: usize, result: Value) {
        if let Some(call) = self.calls.lock().expect("CallLog 锁").get_mut(index) {
            call.result = Some(result);
        }
    }

    pub fn set_error(&self, index: usize, error: impl Into<String>) {
        if let Some(call) = self.calls.lock().expect("CallLog 锁").get_mut(index) {
            call.error = Some(error.into());
        }
    }

    pub fn of(&self, method: &str) -> Vec<Call> {
        self.calls
            .lock()
            .expect("CallLog 锁")
            .iter()
            .filter(|c| c.method == method)
            .cloned()
            .collect()
    }

    pub fn count(&self, method: &str) -> usize {
        self.calls
            .lock()
            .expect("CallLog 锁")
            .iter()
            .filter(|c| c.method == method)
            .count()
    }

    pub fn methods(&self) -> Vec<String> {
        self.calls
            .lock()
            .expect("CallLog 锁")
            .iter()
            .map(|c| c.method.clone())
            .collect()
    }

    pub fn last(&self, method: &str) -> Option<Call> {
        self.calls
            .lock()
            .expect("CallLog 锁")
            .iter()
            .rev()
            .find(|c| c.method == method)
            .cloned()
    }

    pub fn clear(&self) {
        self.calls.lock().expect("CallLog 锁").clear();
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("CallLog 锁").clone()
    }

    pub fn len(&self) -> usize {
        self.calls.lock().expect("CallLog 锁").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Display for CallLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<CallLog {} calls: {:?}>", self.len(), self.methods())
    }
}

/// 造一份 kwargs：`kwargs!{"chat_id" => json!("oc_1"), "limit" => json!(50)}`。
#[macro_export]
macro_rules! kwargs {
    ($($key:expr => $value:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut map = ::serde_json::Map::new();
        $( map.insert($key.to_string(), $value); )*
        map
    }};
}
