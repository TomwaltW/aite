//! `aite evidence show` —— 把 `{evidence_dir}/{task_id}/` 里的证据链渲染成人读的时间线
//! （对应旧 `scripts/evidence_show.py`）。
//!
//! 真机验收（§2.4 的 M1–M6）出问题时没有测试告诉你哪一行断言红了，只有群里一条没回的
//! 消息。每个任务留下的 events.jsonl 是唯一的第一现场，但它一行一个几百字节的 JSON、
//! payload 还只有 hash —— 今天只有机器读得了。这个子命令把它变成人读得懂的时间线。
//!
//! ```text
//! aite evidence show <task_id>                     # 从默认 evidence_dir 找
//! aite evidence show --dir data/evidence/<task_id>
//! aite evidence show --list                        # 列出所有任务（最近写入在前）
//! aite evidence show <task_id> --only model_call,tool_call --tail 20
//! aite evidence show <task_id> --json
//! ```
//!
//! 退出码：0 = 链校验通过；1 = 链断了 / manifest 对不上 / payload 缺失；2 = 找不到任务或参数错。
//!
//! 三条纪律：
//!
//! * **hash 链校验是骨头，不是装饰。** 断了要说清断在第几条、期望什么、实际什么，
//!   并且非零退出码 —— 人在真机排障时最先要知道的就是「这份证据还可不可信」。
//! * **没有 manifest.json 也要能渲染**（任务还在跑、或进程被杀没 finalize），
//!   这种情况打印「未 finalize」，不算损坏。
//! * **不打印密钥、token、消息全文。** 每种 kind 只挑白名单里的字段渲染，
//!   工具参数里键名带 token/secret/key 的一律 `***`。W8 已经保证证据里不存模型全文，
//!   这里不把口子开回来。
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use aite_contracts::{
    AiteConfig, EvidenceEvent, EvidenceKind, GENESIS, chain_hash, payload_hash_of,
};
use chrono::{DateTime, Local, TimeZone, Utc};
use clap::{Args, Parser, Subcommand};
use serde_json::{Map, Value, json};

use crate::writer::{EVENTS_FILE, MANIFEST_FILE};

/// 终态，渲染时要显眼（§2.4 排障第一眼看的就是它）。
const TERMINAL_KINDS: [EvidenceKind; 3] = [
    EvidenceKind::Delivered,
    EvidenceKind::Failed,
    EvidenceKind::Cancelled,
];

/// 工具参数里键名撞上这些词就只打 `***`。证据里本不该有密钥，这是第二道闸。
const SECRET_HINTS: [&str; 8] = [
    "token",
    "secret",
    "password",
    "passwd",
    "api_key",
    "apikey",
    "credential",
    "auth",
];

pub const MARK_TERMINAL: &str = "★";
pub const MARK_BROKEN: &str = "✗";

// ---------------------------------------------------------------- 小工具

fn is_terminal_kind(kind: &str) -> bool {
    TERMINAL_KINDS.iter().any(|k| k.as_str() == kind)
}

/// Python `str(value)`：字符串取原文，别的按 JSON 字面量。
fn display_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `p.get(key, "?")` 那一路：缺了就是 `?`。
fn field_or(p: &Map<String, Value>, key: &str, fallback: &str) -> String {
    p.get(key).map_or_else(|| fallback.to_string(), display_of)
}

fn field_i64(p: &Map<String, Value>, key: &str) -> i64 {
    p.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn keep_of(p: &Map<String, Value>, keys: &[&str]) -> Map<String, Value> {
    let mut out = Map::new();
    for k in keys {
        out.insert((*k).to_string(), p.get(*k).cloned().unwrap_or(Value::Null));
    }
    out
}

fn clip(text: &str, limit: usize) -> String {
    let s = text.replace(['\n', '\r'], " ").trim().to_string();
    if s.chars().count() <= limit {
        return s;
    }
    let mut out: String = s.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// 终端显示宽度 —— 中文是双宽，直接按字符数补空格会把整张表排歪。
fn display_width(text: &str) -> usize {
    text.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
}

fn is_wide(c: char) -> bool {
    // unicodedata.east_asian_width 里 W / F 的主要区段（CJK、假名、韩文、全角形）
    matches!(c as u32,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xA960..=0xA97F | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF | 0xFE10..=0xFE19 | 0xFE30..=0xFE6F | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F | 0x1F900..=0x1F9FF
        | 0x20000..=0x2FFFD | 0x30000..=0x3FFFD)
}

fn pad(text: &str, width: usize) -> String {
    let shown = display_width(text);
    format!("{text}{}", " ".repeat(width.saturating_sub(shown)))
}

fn fmt_elapsed(sec: f64) -> String {
    if sec < 60.0 {
        format!("+{sec:6.2}s")
    } else {
        format!("+{}m{:04.1}s", (sec as i64) / 60, sec % 60.0)
    }
}

fn fmt_money(amount: f64) -> String {
    format!("¥{amount:.4}")
}

/// `{n:,}` —— 千分位。
fn comma(n: i64) -> String {
    let neg = n < 0;
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if neg { format!("-{out}") } else { out }
}

fn round_to(x: f64, digits: u32) -> f64 {
    let f = 10f64.powi(digits as i32);
    (x * f).round() / f
}

fn redact(key: &str, value: &Value) -> String {
    let lower = key.to_lowercase();
    if SECRET_HINTS.iter().any(|h| lower.contains(h)) {
        return "***".to_string();
    }
    match value {
        Value::String(s) => clip(s, 40),
        Value::Array(_) | Value::Object(_) => {
            clip(&serde_json::to_string(value).unwrap_or_default(), 40)
        }
        other => clip(&display_of(other), 40),
    }
}

fn args_summary(arguments: Option<&Value>, limit: usize) -> String {
    let Some(Value::Object(m)) = arguments else {
        return String::new();
    };
    if m.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = m
        .iter()
        .map(|(k, v)| format!("{k}={}", redact(k, v)))
        .collect();
    clip(&parts.join(", "), limit)
}

/// `json.dumps(p, ensure_ascii=False, sort_keys=True)`。
fn dumps_sorted(p: &Map<String, Value>) -> String {
    let sorted: BTreeMap<&String, &Value> = p.iter().collect();
    serde_json::to_string(&sorted).unwrap_or_default()
}

// ---------------------------------------------------------------- 数据形状

/// 链上的一处问题。`line` 是 events.jsonl 的行号（1 起），排障时能直接 `sed -n` 定位。
#[derive(Debug, Clone)]
pub struct ChainIssue {
    pub line: usize,
    pub seq: Option<u64>,
    pub problem: String,
    pub expected: String,
    pub actual: String,
}

impl ChainIssue {
    fn new(line: usize, seq: Option<u64>, problem: impl Into<String>) -> Self {
        Self {
            line,
            seq,
            problem: problem.into(),
            expected: String::new(),
            actual: String::new(),
        }
    }

    fn with(mut self, expected: impl Into<String>, actual: impl Into<String>) -> Self {
        self.expected = expected.into();
        self.actual = actual.into();
        self
    }

    fn as_value(&self) -> Value {
        json!({
            "line": self.line,
            "seq": self.seq,
            "problem": self.problem,
            "expected": self.expected,
            "actual": self.actual,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Row {
    pub seq: u64,
    pub line: usize,
    pub kind: String,
    pub at: Option<DateTime<Utc>>,
    pub elapsed: f64,
    pub detail: String,
    pub fields: Map<String, Value>,
    pub broken: bool,
    pub note: String,
}

impl Row {
    fn as_value(&self) -> Value {
        json!({
            "seq": self.seq,
            "kind": self.kind,
            "at": self.at.map(|t| t.to_rfc3339()),
            "elapsed_sec": round_to(self.elapsed, 3),
            "detail": self.detail,
            "fields": Value::Object(self.fields.clone()),
            "broken": self.broken,
            "note": self.note,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub events: usize,
    pub span_sec: f64,
    pub model_calls: usize,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cost: f64,
    pub tool_calls: usize,
    pub tool_failed: usize,
    pub artifacts: usize,
    pub terminal: String,
    /// payload 拿不到的事件，不进上面任何一个计数
    pub unreadable: usize,
}

impl Stats {
    fn as_value(&self) -> Value {
        json!({
            "events": self.events,
            "span_sec": round_to(self.span_sec, 3),
            "model_calls": self.model_calls,
            "tokens_in": self.tokens_in,
            "tokens_out": self.tokens_out,
            "tokens_total": self.tokens_in + self.tokens_out,
            "cost": round_to(self.cost, 6),
            "tool_calls": self.tool_calls,
            "tool_failed": self.tool_failed,
            "artifacts": self.artifacts,
            "terminal": self.terminal,
            "unreadable": self.unreadable,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Timeline {
    pub task_id: String,
    pub task_dir: PathBuf,
    pub rows: Vec<Row>,
    pub issues: Vec<ChainIssue>,
    pub manifest: Option<Map<String, Value>>,
    pub manifest_exists: bool,
    pub manifest_problems: Vec<String>,
    pub stats: Stats,
    pub checked: usize,
    pub root_hash: String,
}

impl Timeline {
    /// 写过 manifest.json 就算 finalize 过 —— 哪怕它坏了（那是另一个问题，单独报）。
    pub fn finalized(&self) -> bool {
        self.manifest_exists
    }

    /// 链可信 = 没有链上问题，也没有 manifest 对不上。未 finalize 不算不可信。
    pub fn ok(&self) -> bool {
        self.issues.is_empty() && self.manifest_problems.is_empty()
    }

    pub fn as_value(&self, rows: &[Row]) -> Value {
        json!({
            "task_id": self.task_id,
            "dir": self.task_dir.display().to_string(),
            "finalized": self.finalized(),
            "manifest": self.manifest.clone().map(Value::Object),
            "root_hash": self.root_hash,
            "chain": {
                "ok": self.ok(),
                "checked": self.checked,
                "issues": self.issues.iter().map(ChainIssue::as_value).collect::<Vec<_>>(),
                "manifest_problems": self.manifest_problems,
            },
            "summary": self.stats.as_value(),
            "events": rows.iter().map(Row::as_value).collect::<Vec<_>>(),
        })
    }
}

// ---------------------------------------------------------------- 逐 kind 的详情

/// 把 payload 翻成一行人话。checklist 的 id→文本要跨事件记，所以做成有状态的对象。
struct Detailer {
    price_in: f64,
    price_out: f64,
    /// checklist_check/fail 的 payload 只有 id 和 state，没有那一项的文本；文本只在
    /// add 那条里出现过（items 是当时的全量清单，id 形如 c1/c2，位置即编号）。
    /// 真机排障时光看到 "c3 → done" 是没用的，所以在这里把文本补回来。
    checklist: BTreeMap<String, String>,
}

impl Detailer {
    fn new(price_in: f64, price_out: f64) -> Self {
        Self {
            price_in,
            price_out,
            checklist: BTreeMap::new(),
        }
    }

    fn item_text(&self, item_id: &str) -> String {
        match self.checklist.get(item_id) {
            Some(text) if !text.is_empty() => format!("「{}」", clip(text, 24)),
            _ => String::new(),
        }
    }

    fn render(&mut self, kind: &str, p: &Map<String, Value>) -> (String, Map<String, Value>) {
        match kind {
            "task_created" => self.task_created(p),
            "event_received" => self.event_received(p),
            "model_call" => self.model_call(p),
            "tool_call" => self.tool_call(p),
            "tool_result" => self.tool_result(p),
            "checklist_op" => self.checklist_op(p),
            "artifact" => self.artifact(p),
            "delivered" => self.delivered(p),
            "failed" => self.failed(p),
            "cancelled" => self.cancelled(p),
            // 契约以后加了新 kind，也不该让工具哑掉
            _ => (clip(&dumps_sorted(p), 90), Map::new()),
        }
    }

    // -- 建任务 / 收事件 --------------------------------------------------

    fn task_created(&self, p: &Map<String, Value>) -> (String, Map<String, Value>) {
        let keep = keep_of(
            p,
            &["task_no", "title", "created_by", "chat_id", "session_id"],
        );
        let title = clip(&field_or(p, "title", ""), 56);
        let mut bits = vec![field_or(p, "task_no", "")];
        if !title.is_empty() {
            bits.push(format!("「{title}」"));
        }
        bits.push(format!("by={}", field_or(p, "created_by", "?")));
        bits.push(format!("chat={}", field_or(p, "chat_id", "?")));
        let detail = bits
            .into_iter()
            .filter(|b| !b.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        (detail, keep)
    }

    fn event_received(&self, p: &Map<String, Value>) -> (String, Map<String, Value>) {
        let keep = keep_of(
            p,
            &["event_id", "kind", "message_id", "sender_id", "mentioned"],
        );
        let detail = format!(
            "{} msg={} mentioned={} event={}",
            field_or(p, "kind", "?"),
            field_or(p, "message_id", "?"),
            field_or(p, "mentioned", "null"),
            field_or(p, "event_id", "?"),
        );
        (detail, keep)
    }

    // -- 模型 -------------------------------------------------------------

    fn model_call(&self, p: &Map<String, Value>) -> (String, Map<String, Value>) {
        let empty = Map::new();
        let usage = match p.get("usage") {
            Some(Value::Object(m)) => m,
            _ => &empty,
        };
        let tin = field_i64(usage, "input_tokens");
        let tout = field_i64(usage, "output_tokens");
        let cached = field_i64(usage, "cached_tokens");
        // 与 worker 的 _price 同一个式子：元/百万 token
        let cost = (tin as f64 * self.price_in + tout as f64 * self.price_out) / 1_000_000.0;

        let mut keep = Map::new();
        keep.insert(
            "model".into(),
            p.get("model").cloned().unwrap_or(Value::Null),
        );
        keep.insert("step".into(), p.get("step").cloned().unwrap_or(Value::Null));
        keep.insert(
            "finish_reason".into(),
            p.get("finish_reason").cloned().unwrap_or(Value::Null),
        );
        keep.insert("input_tokens".into(), json!(tin));
        keep.insert("output_tokens".into(), json!(tout));
        keep.insert("cached_tokens".into(), json!(cached));
        keep.insert("total_tokens".into(), json!(tin + tout));
        keep.insert("cost".into(), json!(round_to(cost, 6)));

        let cached_bit = if cached != 0 {
            format!(" cached={cached}")
        } else {
            String::new()
        };
        let detail = format!(
            "{} step={} finish={} token in={tin} out={tout} 合计={}{cached_bit} {}",
            field_or(p, "model", "?"),
            field_or(p, "step", "?"),
            field_or(p, "finish_reason", "?"),
            tin + tout,
            fmt_money(cost),
        );
        (detail, keep)
    }

    // -- 工具 -------------------------------------------------------------

    fn tool_call(&self, p: &Map<String, Value>) -> (String, Map<String, Value>) {
        let name = field_or(p, "name", "?");
        let summary = args_summary(p.get("arguments"), 72);
        let mut keep = Map::new();
        keep.insert("name".into(), json!(name));
        keep.insert(
            "call_id".into(),
            p.get("call_id").cloned().unwrap_or(Value::Null),
        );
        keep.insert("arguments_summary".into(), json!(summary));
        (format!("{name}({summary})"), keep)
    }

    fn tool_result(&self, p: &Map<String, Value>) -> (String, Map<String, Value>) {
        let ok = p.get("ok").and_then(Value::as_bool).unwrap_or(false);
        let code = p.get("error").cloned().unwrap_or(Value::Null);
        let ms = p.get("duration_ms").cloned().unwrap_or(json!(0));
        let mut keep = Map::new();
        keep.insert("name".into(), p.get("name").cloned().unwrap_or(Value::Null));
        keep.insert(
            "call_id".into(),
            p.get("call_id").cloned().unwrap_or(Value::Null),
        );
        keep.insert("ok".into(), json!(ok));
        keep.insert("error".into(), code.clone());
        keep.insert("duration_ms".into(), ms.clone());

        let status = if ok {
            "ok".to_string()
        } else {
            let shown = match &code {
                Value::Null => String::new(),
                other => display_of(other),
            };
            format!(
                "FAIL[{}]",
                if shown.is_empty() { "unknown" } else { &shown }
            )
        };
        let detail = format!(
            "{} → {status} {}ms",
            field_or(p, "name", "?"),
            display_of(&ms)
        );
        (detail, keep)
    }

    // -- 进度面 -----------------------------------------------------------

    fn checklist_op(&mut self, p: &Map<String, Value>) -> (String, Map<String, Value>) {
        let op = field_or(p, "op", "?");
        let mut keep = Map::new();
        keep.insert("op".into(), json!(op));

        if op == "add" {
            let items: Vec<String> = match p.get("items") {
                Some(Value::Array(a)) => a.iter().map(display_of).collect(),
                _ => Vec::new(),
            };
            for (idx, text) in items.iter().enumerate() {
                self.checklist.insert(format!("c{}", idx + 1), text.clone());
            }
            let ids: Vec<String> = match p.get("ids") {
                Some(Value::Array(a)) => a.iter().map(display_of).collect(),
                _ => Vec::new(),
            };
            keep.insert("ids".into(), json!(ids));
            keep.insert("items".into(), json!(items));
            let added = if ids.is_empty() {
                "（无）".to_string()
            } else {
                ids.iter()
                    .map(|i| format!("{i}{}", self.item_text(i)))
                    .collect::<Vec<_>>()
                    .join("，")
            };
            return (format!("add {added}"), keep);
        }
        if op == "check" || op == "fail" {
            let item_id = field_or(p, "id", "");
            keep.insert("id".into(), p.get("id").cloned().unwrap_or(Value::Null));
            keep.insert(
                "state".into(),
                p.get("state").cloned().unwrap_or(Value::Null),
            );
            let detail = format!(
                "{op} {item_id} → {} {}",
                field_or(p, "state", "?"),
                self.item_text(&item_id)
            );
            return (detail.trim_end().to_string(), keep);
        }
        if op == "note" {
            keep.insert("text".into(), p.get("text").cloned().unwrap_or(Value::Null));
            return (
                format!("note 「{}」", clip(&field_or(p, "text", ""), 40)),
                keep,
            );
        }
        (format!("{op} {}", clip(&dumps_sorted(p), 70)), keep)
    }

    // -- 产物 -------------------------------------------------------------

    fn artifact(&self, p: &Map<String, Value>) -> (String, Map<String, Value>) {
        let sha = field_or(p, "sha256", "");
        let size = p.get("size").cloned().unwrap_or(json!(0));
        let mut keep = Map::new();
        keep.insert(
            "title".into(),
            p.get("title").cloned().unwrap_or(Value::Null),
        );
        keep.insert("mime".into(), p.get("mime").cloned().unwrap_or(Value::Null));
        keep.insert("size".into(), size.clone());
        keep.insert("sha256".into(), json!(sha));
        let detail = format!(
            "「{}」 {} {}B sha={}",
            clip(&field_or(p, "title", ""), 40),
            field_or(p, "mime", "?"),
            display_of(&size),
            sha.chars().take(8).collect::<String>(),
        );
        (detail, keep)
    }

    // -- 终态 -------------------------------------------------------------

    fn delivered(&self, p: &Map<String, Value>) -> (String, Map<String, Value>) {
        let missing: Vec<Value> = match p.get("missing") {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        let mut keep = Map::new();
        keep.insert(
            "artifacts".into(),
            p.get("artifacts").cloned().unwrap_or(json!(0)),
        );
        keep.insert("missing".into(), json!(missing));
        keep.insert(
            "steps".into(),
            p.get("steps").cloned().unwrap_or(Value::Null),
        );
        let tail = if missing.is_empty() {
            String::new()
        } else {
            format!("，产物缺失 {} 个", missing.len())
        };
        let detail = format!(
            "已交付 · 产出 {} 个 · {} 步{tail}",
            field_or(p, "artifacts", "0"),
            field_or(p, "steps", "?"),
        );
        (detail, keep)
    }

    fn failed(&self, p: &Map<String, Value>) -> (String, Map<String, Value>) {
        let keep = keep_of(p, &["reason", "steps"]);
        let detail = format!(
            "失败 · {} · {} 步",
            clip(&field_or(p, "reason", ""), 72),
            field_or(p, "steps", "?"),
        );
        (detail, keep)
    }

    fn cancelled(&self, p: &Map<String, Value>) -> (String, Map<String, Value>) {
        let keep = keep_of(p, &["by", "steps"]);
        let by = match p.get("by") {
            Some(v) if !display_of(v).is_empty() && *v != Value::Null => {
                format!(" by={}", display_of(v))
            }
            _ => String::new(),
        };
        (
            format!("已取消{by} · {} 步", field_or(p, "steps", "?")),
            keep,
        )
    }
}

// ---------------------------------------------------------------- 读 + 校验

fn load_manifest(task_dir: &Path) -> (Option<Map<String, Value>>, Vec<String>) {
    let path = task_dir.join(MANIFEST_FILE);
    if !path.exists() {
        return (None, Vec::new());
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return (None, vec![format!("manifest.json 读不了：{e}")]),
    };
    match serde_json::from_str::<Value>(&text) {
        Err(e) => (None, vec![format!("manifest.json 解析不了：{e}")]),
        Ok(Value::Object(m)) => (Some(m), Vec::new()),
        Ok(other) => {
            let kind = match other {
                Value::Array(_) => "list",
                Value::String(_) => "str",
                Value::Number(_) => "number",
                Value::Bool(_) => "bool",
                _ => "null",
            };
            (None, vec![format!("manifest.json 顶层不是对象，是 {kind}")])
        }
    }
}

/// 返回 (payload, 说明)。payload 为 None 时说明里写清为什么拿不到。
fn resolve_payload(task_dir: &Path, ev: &EvidenceEvent) -> (Option<Map<String, Value>>, String) {
    if let Some(p) = &ev.payload {
        return (Some(p.clone()), String::new());
    }
    let Some(rel) = &ev.payload_ref else {
        return (None, "payload 既没内联也没 payload_ref".to_string());
    };
    let path = task_dir.join(rel);
    if !path.exists() {
        return (None, format!("payload 缺失（{rel} 不在）"));
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return (None, format!("外置 payload 读不了（{rel}）：{e}")),
    };
    match serde_json::from_str::<Value>(&text) {
        Err(e) => (None, format!("外置 payload 解析不了（{rel}）：{e}")),
        Ok(Value::Object(m)) => (Some(m), format!("外置 payload {rel}")),
        Ok(_) => (None, format!("外置 payload 顶层不是对象（{rel}）")),
    }
}

/// 读一个任务目录，逐条重算 hash 链，产出可渲染的时间线。
///
/// 校验口径和 `FileEvidenceWriter::verify` 一致（seq 连续、payload 与 payload_hash 对得上、
/// prev_hash 接得住、hash == chain_hash(prev, payload_hash)），差别是这里不提前返回：
/// 每条都查完、把问题全收起来，人才知道是断了一处还是整份都不对。
/// 往下走时 prev 用**落盘那条**的 hash，这样篡改一条只报一处，不会级联出一串假问题。
pub fn load_timeline(task_dir: &Path, price_in: f64, price_out: f64) -> Timeline {
    let events_path = task_dir.join(EVENTS_FILE);
    let (manifest, manifest_problems) = load_manifest(task_dir);
    let mut tl = Timeline {
        task_id: task_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        task_dir: task_dir.to_path_buf(),
        rows: Vec::new(),
        issues: Vec::new(),
        manifest: manifest.clone(),
        manifest_exists: task_dir.join(MANIFEST_FILE).exists(),
        manifest_problems,
        stats: Stats::default(),
        checked: 0,
        root_hash: GENESIS.to_string(),
    };

    let mut detailer = Detailer::new(price_in, price_out);
    // 不能用 unwrap_or_default()：events.jsonl 不是合法 UTF-8 时那会把它当成空文件，
    // 于是「0 条事件 · 链 OK」退出 0 —— 而 FileEvidenceWriter::verify 对同一份文件判 false。
    // 崩溃现场（写到一半被杀、残行截在多字节字符中间、还没 finalize）正好命中这条路，
    // 而这个子命令存在的全部理由就是在那时候说真话。
    let raw = match std::fs::read(&events_path).map(String::from_utf8) {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => {
            tl.issues.push(ChainIssue::new(
                0,
                None,
                format!("events.jsonl 不是合法的 UTF-8（{e}）—— 多半是上一个进程写到一半被杀"),
            ));
            String::new()
        }
        Err(e) => {
            tl.issues.push(ChainIssue::new(
                0,
                None,
                format!("读不了 events.jsonl：{e}"),
            ));
            String::new()
        }
    };
    let mut prev = GENESIS.to_string();
    let mut first_at: Option<DateTime<Utc>> = None;
    let mut first_task_id: Option<String> = None;
    let mut index: u64 = 0; // 期望的 seq，跳过空行后连续

    for (line_no, line) in raw.lines().enumerate().map(|(i, l)| (i + 1, l)) {
        if line.trim().is_empty() {
            continue;
        }
        let ev: EvidenceEvent = match serde_json::from_str(line) {
            Ok(ev) => ev,
            Err(e) => {
                tl.issues.push(ChainIssue::new(
                    line_no,
                    None,
                    format!("这一行不是合法的 EvidenceEvent：{e}"),
                ));
                index += 1;
                continue;
            }
        };

        tl.checked += 1;
        let mut broken = false;

        // task_id 以第一条为准（--dir 指到一个改过名的目录时，目录名不作数）；
        // 同一个文件里混进别的任务，那是串目录了，得报出来
        match &first_task_id {
            None => {
                first_task_id = Some(ev.task_id.clone());
                tl.task_id = ev.task_id.clone();
            }
            Some(first) if *first != ev.task_id => {
                tl.issues.push(
                    ChainIssue::new(
                        line_no,
                        Some(ev.seq),
                        "这一条的 task_id 和文件里其他事件不是同一个任务",
                    )
                    .with(first.clone(), ev.task_id.clone()),
                );
                broken = true;
            }
            Some(_) => {}
        }

        if ev.seq != index {
            tl.issues.push(
                ChainIssue::new(line_no, Some(ev.seq), "seq 不连续")
                    .with(index.to_string(), ev.seq.to_string()),
            );
            broken = true;
        }

        let (payload, note) = resolve_payload(task_dir, &ev);
        match &payload {
            None => {
                let problem = if note.is_empty() {
                    "payload 拿不到"
                } else {
                    &note
                };
                tl.issues
                    .push(ChainIssue::new(line_no, Some(ev.seq), problem));
                broken = true;
            }
            Some(p) => {
                let actual_hash = payload_hash_of(p);
                if actual_hash != ev.payload_hash {
                    tl.issues.push(
                        ChainIssue::new(
                            line_no,
                            Some(ev.seq),
                            "payload 与 payload_hash 对不上（内容被改过）",
                        )
                        .with(ev.payload_hash.clone(), actual_hash),
                    );
                    broken = true;
                }
            }
        }

        if ev.prev_hash != prev {
            tl.issues.push(
                ChainIssue::new(line_no, Some(ev.seq), "prev_hash 接不住上一条")
                    .with(prev.clone(), ev.prev_hash.clone()),
            );
            broken = true;
        }

        let want_hash = chain_hash(&ev.prev_hash, &ev.payload_hash);
        if ev.hash != want_hash {
            tl.issues.push(
                ChainIssue::new(
                    line_no,
                    Some(ev.seq),
                    "hash != chain_hash(prev_hash, payload_hash)",
                )
                .with(want_hash, ev.hash.clone()),
            );
            broken = true;
        }

        if first_at.is_none() {
            first_at = Some(ev.created_at);
        }
        let elapsed = first_at.map_or(0.0, |f| {
            (ev.created_at - f).num_microseconds().unwrap_or(0) as f64 / 1e6
        });

        let kind = ev.kind.as_str().to_string();
        let empty = Map::new();
        let (mut detail, fields) = detailer.render(&kind, payload.as_ref().unwrap_or(&empty));
        if payload.is_none() {
            detail = if note.is_empty() {
                "payload 拿不到，无法渲染".to_string()
            } else {
                note.clone()
            };
        }
        let readable = payload.is_some();
        tl.rows.push(Row {
            seq: ev.seq,
            line: line_no,
            kind: kind.clone(),
            at: Some(ev.created_at),
            elapsed,
            detail,
            fields: fields.clone(),
            broken,
            note: if readable && !note.is_empty() {
                note
            } else {
                String::new()
            },
        });
        accumulate(&mut tl.stats, &kind, &fields, readable);
        prev = ev.hash;
        index += 1;
    }

    tl.root_hash = prev;
    tl.stats.events = tl.rows.len();
    tl.stats.span_sec = tl.rows.last().map_or(0.0, |r| r.elapsed);
    if let Some(last) = tl.rows.last()
        && is_terminal_kind(&last.kind)
    {
        tl.stats.terminal = last.kind.clone();
    }

    if let Some(m) = &manifest {
        tl.manifest_problems.extend(check_manifest(m, &tl));
    }
    tl
}

fn accumulate(stats: &mut Stats, kind: &str, fields: &Map<String, Value>, readable: bool) {
    if !readable {
        // payload 读不到就什么都别算。不然一条丢了 payload 的 tool_result 会被当成
        // 「工具失败一次」—— 真机排障时这种假失败最误事。
        stats.unreadable += 1;
        return;
    }
    match kind {
        "model_call" => {
            stats.model_calls += 1;
            stats.tokens_in += field_i64(fields, "input_tokens");
            stats.tokens_out += field_i64(fields, "output_tokens");
            stats.cost += fields.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
        }
        "tool_call" => stats.tool_calls += 1,
        "tool_result" => {
            if !fields.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                stats.tool_failed += 1;
            }
        }
        "artifact" => stats.artifacts += 1,
        _ => {}
    }
}

fn check_manifest(manifest: &Map<String, Value>, tl: &Timeline) -> Vec<String> {
    let mut problems = Vec::new();
    let root = manifest.get("root_hash");
    if root.and_then(Value::as_str) != Some(tl.root_hash.as_str()) {
        let shown = root.map_or_else(|| "（没有这个字段）".to_string(), display_of);
        problems.push(format!(
            "manifest.root_hash 与最后一条事件的 hash 对不上：manifest={shown} 实际={}",
            tl.root_hash
        ));
    }
    if let Some(count) = manifest.get("event_count").and_then(Value::as_i64)
        && count != tl.rows.len() as i64
    {
        problems.push(format!(
            "manifest.event_count={count}，events.jsonl 实际 {} 条",
            tl.rows.len()
        ));
    }
    if let Some(mid) = manifest.get("task_id").and_then(Value::as_str)
        && !mid.is_empty()
        && !tl.rows.is_empty()
        && mid != tl.task_id
    {
        problems.push(format!(
            "manifest.task_id={mid}，事件里的 task_id={}",
            tl.task_id
        ));
    }
    problems
}

// ---------------------------------------------------------------- 渲染

pub fn filter_rows(rows: &[Row], only: Option<&BTreeSet<String>>, tail: Option<usize>) -> Vec<Row> {
    let mut out: Vec<Row> = rows
        .iter()
        .filter(|r| only.is_none_or(|o| o.contains(&r.kind)))
        .cloned()
        .collect();
    if let Some(n) = tail {
        if n == 0 {
            out.clear();
        } else if out.len() > n {
            out = out.split_off(out.len() - n);
        }
    }
    out
}

pub fn render_text(tl: &Timeline, rows: &[Row], filtered: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("任务 {}", tl.task_id));
    lines.push(format!("目录 {}", tl.task_dir.display()));
    match &tl.manifest {
        Some(m) => {
            let bits: Vec<String> = [
                "session_id",
                "task_no",
                "created_by",
                "model",
                "contract_version",
            ]
            .iter()
            // Python：`if m.get(k) not in (None, "")`
            .filter(|k| {
                m.get(**k)
                    .is_some_and(|v| !v.is_null() && !display_of(v).is_empty())
            })
            .map(|k| format!("{k}={}", display_of(&m[*k])))
            .collect();
            lines.push(format!("manifest  {}", bits.join(" ")));
        }
        None if tl.manifest_exists => lines.push(
            "manifest  有 manifest.json 但读不了（见下面的链校验），任务信息取不到".to_string(),
        ),
        None => lines.push(
            "manifest  未 finalize（没有 manifest.json —— 任务可能还在跑，或进程被杀了）"
                .to_string(),
        ),
    }
    lines.push(String::new());

    if tl.rows.is_empty() {
        lines.push("（events.jsonl 里没有事件）".to_string());
    } else {
        if filtered {
            lines.push(format!(
                "（已过滤：显示 {}/{} 条；汇总与链校验仍按全量算）",
                rows.len(),
                tl.rows.len()
            ));
        }
        lines.push("     seq       耗时  kind             详情".to_string());
        lines.push(format!("  {}", "─".repeat(106)));
        for r in rows {
            let mark = if r.broken {
                MARK_BROKEN
            } else if is_terminal_kind(&r.kind) {
                MARK_TERMINAL
            } else {
                " "
            };
            lines.push(format!(
                "{mark} {:5}  {}  {:<16} {}",
                r.seq,
                fmt_elapsed(r.elapsed),
                r.kind,
                r.detail
            ));
            if !r.note.is_empty() {
                lines.push(format!("        └─ {}", r.note));
            }
        }
    }

    lines.push(String::new());
    lines.push(format!("── 汇总 {}", "─".repeat(98)));
    let s = &tl.stats;
    let span = fmt_elapsed(s.span_sec)
        .trim_start_matches('+')
        .trim()
        .to_string();
    let tail = if s.terminal.is_empty() {
        "，无终态事件（任务没走到 delivered/failed/cancelled）".to_string()
    } else {
        format!("，终态 {}", s.terminal)
    };
    lines.push(format!("事件      {} 条，跨度 {span}{tail}", s.events));
    lines.push(format!(
        "模型调用  {} 次 · token in={} out={} 合计={} · 花费 {}",
        s.model_calls,
        comma(s.tokens_in),
        comma(s.tokens_out),
        comma(s.tokens_in + s.tokens_out),
        fmt_money(s.cost),
    ));
    lines.push(format!(
        "工具调用  {} 次（失败 {} 次）",
        s.tool_calls, s.tool_failed
    ));
    lines.push(format!("产出文件  {} 个", s.artifacts));
    if s.unreadable > 0 {
        lines.push(format!(
            "读不到    {} 条事件的 payload 拿不到，上面几行的数字不含它们",
            s.unreadable
        ));
    }
    lines.extend(render_chain(tl));
    lines.join("\n")
}

fn render_chain(tl: &Timeline) -> Vec<String> {
    let mut lines = Vec::new();
    if tl.ok() {
        let tail = if tl.manifest.is_some() {
            "，root_hash 与 manifest 一致"
        } else {
            "（未 finalize，无 root_hash 可比）"
        };
        lines.push(format!("hash 链   OK · {} 条全部闭合{tail}", tl.checked));
        lines.push(format!("          root {}", tl.root_hash));
        return lines;
    }

    lines.push(format!(
        "hash 链   断了 {MARK_BROKEN} · 查了 {} 条，发现 {} 处问题",
        tl.checked,
        tl.issues.len()
    ));
    for issue in &tl.issues {
        let where_ = match issue.seq {
            Some(seq) => format!("第 {} 行（seq={seq}）", issue.line),
            None => format!("第 {} 行", issue.line),
        };
        lines.push(format!("          {where_}：{}", issue.problem));
        if !issue.expected.is_empty() || !issue.actual.is_empty() {
            lines.push(format!("            期望 {}", issue.expected));
            lines.push(format!("            实际 {}", issue.actual));
        }
    }
    for problem in &tl.manifest_problems {
        lines.push(format!("          manifest：{problem}"));
    }
    lines.push(
        "          → 这份证据不可信，别拿它当验收依据；先确认目录有没有被人手改过。".to_string(),
    );
    lines
}

// ---------------------------------------------------------------- --list

#[derive(Debug, Clone)]
pub struct TaskRow {
    pub task_id: String,
    pub dir: PathBuf,
    pub mtime: f64,
    pub events: usize,
    pub terminal: String,
    pub finalized: bool,
    pub chain_ok: bool,
}

impl TaskRow {
    fn as_value(&self) -> Value {
        json!({
            "task_id": self.task_id,
            "dir": self.dir.display().to_string(),
            "mtime": self.mtime,
            "events": self.events,
            "terminal": self.terminal,
            "finalized": self.finalized,
            "chain_ok": self.chain_ok,
        })
    }
}

pub fn list_tasks(root: &Path, price_in: f64, price_out: f64) -> Vec<TaskRow> {
    if !root.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let d = entry.path();
        let events = d.join(EVENTS_FILE);
        if !d.is_dir() || !events.exists() {
            continue;
        }
        let tl = load_timeline(&d, price_in, price_out);
        let mtime = std::fs::metadata(&events)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0.0, |d| d.as_secs_f64());
        found.push(TaskRow {
            task_id: tl.task_id.clone(),
            dir: d,
            mtime,
            events: tl.stats.events,
            terminal: tl.stats.terminal.clone(),
            finalized: tl.finalized(),
            chain_ok: tl.ok(),
        });
    }
    found.sort_by(|a, b| {
        b.mtime
            .partial_cmp(&a.mtime)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    found
}

pub fn render_list(root: &Path, rows: &[TaskRow]) -> String {
    let mut lines = vec![
        format!("证据根目录 {}（{} 个任务）", root.display(), rows.len()),
        String::new(),
    ];
    if !root.is_dir() {
        lines.push("（这个目录不存在 —— 对一下 config 里的 storage.evidence_dir，".to_string());
        lines.push("  以及起飞日志里打出来的那个 evidence 目录）".to_string());
        return lines.join("\n");
    }
    if rows.is_empty() {
        lines.push("（目录在，但里面没有任何带 events.jsonl 的任务子目录）".to_string());
        return lines.join("\n");
    }
    lines.push(format!(
        "  最后写入             {}  事件  {} {} 链",
        pad("任务", 32),
        pad("终态", 12),
        pad("manifest", 12)
    ));
    lines.push(format!("  {}", "─".repeat(92)));
    for r in rows {
        let when = Local.timestamp_opt(r.mtime as i64, 0).single().map_or_else(
            || "?".to_string(),
            |t| t.format("%Y-%m-%d %H:%M:%S").to_string(),
        );
        let state = if r.terminal.is_empty() {
            "无终态"
        } else {
            &r.terminal
        };
        let final_ = if r.finalized {
            "已写"
        } else {
            "未 finalize"
        };
        let chain = if r.chain_ok {
            "OK".to_string()
        } else {
            format!("断了 {MARK_BROKEN}")
        };
        lines.push(format!(
            "  {when}  {}  {:>4}  {} {} {chain}",
            pad(&r.task_id, 32),
            r.events,
            pad(state, 12),
            pad(final_, 12)
        ));
    }
    let broken: Vec<&TaskRow> = rows.iter().filter(|r| !r.chain_ok).collect();
    if !broken.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "{MARK_BROKEN} 有 {} 个任务的证据链不可信：{}",
            broken.len(),
            broken
                .iter()
                .map(|r| r.task_id.as_str())
                .collect::<Vec<_>>()
                .join("、")
        ));
        lines.push("  逐个跑 aite evidence show <task_id> 看断在哪一条。".to_string());
    }
    lines.join("\n")
}

// ---------------------------------------------------------------- CLI

/// 读 config 只为拿 evidence_dir 和单价。读不到就用契约默认值，并说明一句。
fn load_config(path: &Path) -> (AiteConfig, String) {
    if !path.exists() {
        return (AiteConfig::default(), format!("{} 不在", path.display()));
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            return (
                AiteConfig::default(),
                format!("{} 读不了：{e}", path.display()),
            );
        }
    };
    // 配置坏了不该让排障工具也跟着哑
    match AiteConfig::from_yaml_str(&text) {
        Ok(cfg) => (cfg, String::new()),
        Err(e) => (
            AiteConfig::default(),
            format!("{} 读不了：{e}", path.display()),
        ),
    }
}

/// 只在配置真的影响输出时才唠叨一句 —— 单价没配，花费那一栏就是假的 0。
fn config_note(cfg_note: &str, price_in: f64, price_out: f64) -> String {
    if price_in == 0.0 && price_out == 0.0 {
        let why = if cfg_note.is_empty() {
            "config 里 price_in/out_per_mtok 都是 0；".to_string()
        } else {
            format!("{cfg_note}；")
        };
        return format!("提示：{why}花费一栏按 0 元算，不代表真没花钱（用 --config 指到真配置）");
    }
    if cfg_note.is_empty() {
        String::new()
    } else {
        format!("提示：{cfg_note}，evidence_dir 与单价用契约默认值")
    }
}

#[derive(Parser)]
#[command(
    name = "aite evidence",
    about = "把任务的证据链渲染成人读的时间线，并校验 hash 链。",
    after_help = "例子：\n  \
        aite evidence show --list\n  \
        aite evidence show tsk_0a1b2c\n  \
        aite evidence show --dir data/evidence/tsk_0a1b2c --only model_call,tool_call\n  \
        aite evidence show tsk_0a1b2c --tail 20 --json\n\n\
        退出码：0 = 链校验通过；1 = 链断了 / manifest 对不上 / payload 缺失；2 = 找不到任务或参数错。"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 渲染并校验一个任务的证据链（或 `--list` 列出全部）
    Show(ShowArgs),
}

#[derive(Args)]
struct ShowArgs {
    /// 任务 id（在 --root 下找同名目录）
    task_id: Option<String>,
    /// 直接指向某个任务目录（里面有 events.jsonl）
    #[arg(long)]
    dir: Option<PathBuf>,
    /// 证据根目录；默认取 config 里的 storage.evidence_dir
    #[arg(long)]
    root: Option<PathBuf>,
    /// 配置文件，用来取 evidence_dir 与单价
    #[arg(long, default_value = "config/aite.yaml")]
    config: PathBuf,
    /// 列出根目录下所有任务（最近写入在前）
    #[arg(long)]
    list: bool,
    /// 只显示这些 kind，逗号分隔（如 model_call,tool_call）
    #[arg(long)]
    only: Option<String>,
    /// 只显示最后 N 条
    #[arg(long, allow_hyphen_values = true)]
    tail: Option<i64>,
    /// 输出机器可读的 JSON
    #[arg(long)]
    json: bool,
}

/// 子命令入口：收到的是 `aite evidence` 之后的全部参数。返回进程退出码。
pub fn run(args: Vec<String>) -> i32 {
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    run_with(args, &mut out, &mut err)
}

/// 同 [`run`]，但 stdout / stderr 可注入（测试用）。
pub fn run_with(args: Vec<String>, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    // argv[0] 决定 usage 行怎么印 —— 用户敲的是 `aite evidence …`
    let argv = std::iter::once("aite evidence".to_string()).chain(args);
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) => {
            let text = e.render().to_string();
            // 只有人主动要 --help / --version 才是 0；缺子命令属于参数错，照 2 走
            return if matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                let _ = write!(out, "{text}");
                0
            } else {
                let _ = write!(err, "{text}");
                2
            };
        }
    };
    match cli.cmd {
        Cmd::Show(args) => show(args, out, err),
    }
}

fn show(args: ShowArgs, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let (cfg, cfg_note) = load_config(&args.config);
    let (price_in, price_out) = (cfg.model.price_in_per_mtok, cfg.model.price_out_per_mtok);

    if args.list {
        if args.dir.is_some() {
            let _ = writeln!(
                err,
                "--list 要的是证据根目录，用 --root；--dir 是单个任务目录。"
            );
            return 2;
        }
        let root = args
            .root
            .clone()
            .unwrap_or_else(|| PathBuf::from(&cfg.storage.evidence_dir));
        let rows = list_tasks(&root, price_in, price_out);
        if args.json {
            let payload = json!({
                "root": root.display().to_string(),
                "tasks": rows.iter().map(TaskRow::as_value).collect::<Vec<_>>(),
            });
            let _ = writeln!(
                out,
                "{}",
                serde_json::to_string_pretty(&payload).unwrap_or_default()
            );
        } else {
            if !cfg_note.is_empty() && args.root.is_none() {
                let _ = writeln!(out, "提示：{cfg_note}，证据根目录用契约默认值");
            }
            let _ = writeln!(out, "{}", render_list(&root, &rows));
        }
        // 有任何一份证据链断了就非零退出 —— --list 常被拿来一眼扫全场
        return if rows.iter().all(|r| r.chain_ok) {
            0
        } else {
            1
        };
    }

    let task_dir = if let Some(d) = &args.dir {
        d.clone()
    } else if let Some(t) = &args.task_id {
        let root = args
            .root
            .clone()
            .unwrap_or_else(|| PathBuf::from(&cfg.storage.evidence_dir));
        root.join(t)
    } else {
        let _ = writeln!(
            err,
            "要给一个 task_id，或用 --dir 指向任务目录，或用 --list 看有哪些任务。"
        );
        return 2;
    };

    if !task_dir.join(EVENTS_FILE).exists() {
        let _ = writeln!(
            err,
            "找不到证据：{} 不在。",
            task_dir.join(EVENTS_FILE).display()
        );
        if !task_dir.exists() {
            let _ = writeln!(
                err,
                "（目录 {} 本身就不存在；--list 看看根目录下有哪些任务）",
                task_dir.display()
            );
        }
        return 2;
    }

    if let Some(n) = args.tail
        && n < 0
    {
        let _ = writeln!(err, "--tail 要一个 >= 0 的数。");
        return 2;
    }

    let mut only: Option<BTreeSet<String>> = None;
    if let Some(spec) = &args.only {
        let valid: BTreeSet<String> = EvidenceKind::ALL
            .iter()
            .map(|k| k.as_str().to_string())
            .collect();
        let want: BTreeSet<String> = spec
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        let bad: Vec<&String> = want.difference(&valid).collect();
        if !bad.is_empty() {
            let _ = writeln!(
                err,
                "--only 里有不认识的 kind：{}",
                bad.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let _ = writeln!(
                err,
                "可用：{}",
                valid
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            return 2;
        }
        only = Some(want);
    }

    let tl = load_timeline(&task_dir, price_in, price_out);
    let rows = filter_rows(&tl.rows, only.as_ref(), args.tail.map(|n| n as usize));

    if args.json {
        let _ = writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&tl.as_value(&rows)).unwrap_or_default()
        );
    } else {
        let note = config_note(&cfg_note, price_in, price_out);
        if !note.is_empty() {
            let _ = writeln!(out, "{note}");
        }
        let _ = writeln!(
            out,
            "{}",
            render_text(&tl, &rows, rows.len() != tl.rows.len())
        );
    }
    if tl.ok() { 0 } else { 1 }
}
