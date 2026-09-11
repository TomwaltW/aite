//! 场景 `expect:` 的断言 DSL（对应旧 `aite/evals/checks.py`）。
//!
//! 每条断言是一个 mapping，`check` 选类型，其余键是参数。检查函数返回 `Ok(None)` 表示
//! 通过，`Ok(Some(原因))` 表示没过（要写清「期望什么、实际什么」，报告里就靠这句话），
//! `Err(CheckError)` 表示断言本身写错了 —— 两者都只报不抛。
//!
//! 比较子统一是 `equals` / `min` / `max`，可以同时给。例：
//!
//! ```yaml
//! expect:
//!   - {check: platform_calls, method: send_card, equals: 1}
//!   - {check: cards, distinct_equals: 1}
//!   - {check: text, where: last, contains: 完成}
//!   - {check: file, index: 0, magic: 89504e470d0a1a0a}
//!   - {check: distinct_matches, source: last_text, pattern: 'om_[a-z0-9]+', min: 3}
//! ```
use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::deps::Deps;
use crate::regex_mini::Regex;

/// 断言本身写错了（未知 check、缺参数）—— 与「断言没通过」区分开。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CheckError(pub String);

type Outcome = Result<Option<String>, CheckError>;

/// 全部 check 名字（`--list` 与错误消息里要列）。
pub const CHECK_NAMES: [&str; 12] = [
    "cards",
    "distinct_matches",
    "evidence",
    "file",
    "gateway_calls",
    "gateway_result",
    "model_calls",
    "model_tools",
    "outbound_total",
    "platform_calls",
    "sandbox_calls",
    "store",
    // NOTE: `task` 与 `text` 见下面的 EXTRA_CHECK_NAMES（数组长度是编译期常量，
    // 两段拼起来才是全集）
];

pub const EXTRA_CHECK_NAMES: [&str; 2] = ["task", "text"];

fn all_check_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = CHECK_NAMES.into_iter().chain(EXTRA_CHECK_NAMES).collect();
    names.sort_unstable();
    names
}

fn has_any(spec: &Map<String, Value>, keys: &[&str]) -> bool {
    keys.iter().any(|k| spec.contains_key(*k))
}

fn as_i64(spec: &Map<String, Value>, key: &str) -> Option<i64> {
    spec.get(key).and_then(Value::as_i64)
}

/// 按 equals / min / max 比一个整数。一个都没给就是断言写漏了。
fn compare(label: &str, actual: i64, spec: &Map<String, Value>) -> Outcome {
    if !has_any(spec, &["equals", "min", "max"]) {
        return Err(CheckError(format!(
            "{label}: 至少要给 equals / min / max 之一"
        )));
    }
    if let Some(want) = as_i64(spec, "equals")
        && actual != want
    {
        return Ok(Some(format!("{label} 期望 == {want}，实际 {actual}")));
    }
    if let Some(want) = as_i64(spec, "min")
        && actual < want
    {
        return Ok(Some(format!("{label} 期望 >= {want}，实际 {actual}")));
    }
    if let Some(want) = as_i64(spec, "max")
        && actual > want
    {
        return Ok(Some(format!("{label} 期望 <= {want}，实际 {actual}")));
    }
    Ok(None)
}

fn need<'a>(spec: &'a Map<String, Value>, key: &str) -> Result<&'a Value, CheckError> {
    spec.get(key).ok_or_else(|| {
        let name = spec.get("check").cloned().unwrap_or(Value::Null);
        CheckError(format!("check={name} 缺少必填键 {key:?}"))
    })
}

fn need_str(spec: &Map<String, Value>, key: &str) -> Result<String, CheckError> {
    Ok(text_of(need(spec, key)?))
}

/// 取值转成字符串：字符串原样，别的走 JSON（`contains: 1` 这类写法也能用）。
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 跑一条断言。
pub fn run_check(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    let name = spec
        .get("check")
        .and_then(Value::as_str)
        .ok_or_else(|| CheckError("expect 条目缺 check 键".to_string()))?;
    match name {
        "platform_calls" => platform_calls(deps, spec),
        "outbound_total" => outbound_total(deps, spec),
        "cards" => cards(deps, spec),
        "text" => text_check(deps, spec),
        "distinct_matches" => distinct_matches(deps, spec),
        "file" => file_check(deps, spec),
        "gateway_calls" => gateway_calls(deps, spec),
        "gateway_result" => gateway_result(deps, spec),
        "model_tools" => model_tools(deps, spec),
        "model_calls" => model_calls(deps, spec),
        "sandbox_calls" => sandbox_calls(deps, spec),
        "store" => store_check(deps, spec),
        "task" => task_check(deps, spec),
        "evidence" => evidence_check(deps, spec),
        other => Err(CheckError(format!(
            "未知的 check={other:?}，可用：{:?}",
            all_check_names()
        ))),
    }
}

/// 出站/入站方法被调了几次。method 取 send_text / send_card / update_card /
/// send_file / add_reaction / read_history / read_document / download_file。
fn platform_calls(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    let method = need_str(spec, "method")?;
    compare(
        &format!("platform.{method}"),
        deps.platform.calls.count(&method) as i64,
        spec,
    )
}

/// 所有出站动作之和。09_bot_ignored 用 `{equals: 0}`。
fn outbound_total(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    compare("出站动作总数", deps.platform.outbound_count() as i64, spec)
}

/// 卡片：发了几张（distinct_equals）、更新了几次（updates_min / updates_equals）、
/// 最后一张的终态（final_status）。
fn cards(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    if let Some(want) = as_i64(spec, "distinct_equals")
        && deps.platform.card_count() as i64 != want
    {
        return Ok(Some(format!(
            "卡片张数期望 == {want}，实际 {}（card_id：{:?}）",
            deps.platform.card_count(),
            deps.platform.card_ids()
        )));
    }
    if let Some(want) = as_i64(spec, "updates_min")
        && (deps.platform.update_count() as i64) < want
    {
        return Ok(Some(format!(
            "update_card 次数期望 >= {want}，实际 {}",
            deps.platform.update_count()
        )));
    }
    if let Some(want) = as_i64(spec, "updates_equals")
        && deps.platform.update_count() as i64 != want
    {
        return Ok(Some(format!(
            "update_card 次数期望 == {want}，实际 {}",
            deps.platform.update_count()
        )));
    }
    if let Some(want) = spec.get("final_status") {
        let want = text_of(want);
        let snaps = deps.platform.card_snapshots(None);
        let Some(last) = snaps.last() else {
            return Ok(Some(format!("卡片终态期望 {want}，实际一张卡片都没发")));
        };
        if last.status.as_str() != want {
            return Ok(Some(format!(
                "卡片终态期望 {want}，实际 {}",
                last.status.as_str()
            )));
        }
    }
    if !has_any(
        spec,
        &[
            "distinct_equals",
            "updates_min",
            "updates_equals",
            "final_status",
        ],
    ) {
        return Err(CheckError(
            "check=cards 至少要给 distinct_equals / updates_min / updates_equals / final_status"
                .to_string(),
        ));
    }
    Ok(None)
}

/// 发出去的文本消息。where: any（默认）/ last / all；contains / not_contains / matches。
fn text_check(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    let texts = deps.platform.texts();
    let where_ = spec.get("where").map(text_of).unwrap_or("any".to_string());
    let pool: Vec<String> = match where_.as_str() {
        "last" => texts.last().cloned().into_iter().collect(),
        "any" | "all" => texts.clone(),
        other => {
            return Err(CheckError(format!(
                "check=text 的 where 只能是 any / last / all，收到 {other:?}"
            )));
        }
    };
    let shown = if pool.is_empty() {
        "(一条都没有)".to_string()
    } else {
        format!("{pool:?}")
    };

    if let Some(want) = spec.get("contains") {
        let want = text_of(want);
        let hit: Vec<&String> = pool.iter().filter(|t| t.contains(&want)).collect();
        let ok = if where_ == "all" {
            !pool.is_empty() && hit.len() == pool.len()
        } else {
            !hit.is_empty()
        };
        if !ok {
            let every = if where_ == "all" { "每条" } else { "" };
            return Ok(Some(format!(
                "没有{every}文本包含 {want:?}；实际文本：{shown}"
            )));
        }
    }
    if let Some(bad) = spec.get("not_contains") {
        let bad = text_of(bad);
        let hit: Vec<&String> = pool.iter().filter(|t| t.contains(&bad)).collect();
        if !hit.is_empty() {
            return Ok(Some(format!("文本不该包含 {bad:?}，但出现在：{hit:?}")));
        }
    }
    if let Some(pattern) = spec.get("matches") {
        let raw = text_of(pattern);
        let re = Regex::new(&raw).map_err(|e| CheckError(format!("check=text 的 matches：{e}")))?;
        if !pool.iter().any(|t| re.is_match(t)) {
            return Ok(Some(format!("没有文本匹配 /{raw}/；实际文本：{shown}")));
        }
    }
    if !has_any(spec, &["contains", "not_contains", "matches"]) {
        return Err(CheckError(
            "check=text 至少要给 contains / not_contains / matches".to_string(),
        ));
    }
    Ok(None)
}

/// 在文本里数「不重复的匹配」有几个。05 用它验回复引用了 >=3 个 message_id。
fn distinct_matches(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    let source = spec
        .get("source")
        .map(text_of)
        .unwrap_or("last_text".to_string());
    let texts = deps.platform.texts();
    let pool: Vec<String> = match source.as_str() {
        "last_text" => texts.last().cloned().into_iter().collect(),
        "all_texts" => texts.clone(),
        other => {
            return Err(CheckError(format!(
                "check=distinct_matches 的 source 只能是 last_text / all_texts，收到 {other:?}"
            )));
        }
    };
    let raw = need_str(spec, "pattern")?;
    let re = Regex::new(&raw)
        .map_err(|e| CheckError(format!("check=distinct_matches 的 pattern：{e}")))?;
    let mut found: BTreeSet<String> = BTreeSet::new();
    for t in &pool {
        found.extend(re.find_all(t));
    }
    let shown = if pool.is_empty() {
        "(一条都没有)".to_string()
    } else {
        format!("{pool:?}")
    };
    let result = compare(&format!("/{raw}/ 的不重复命中数"), found.len() as i64, spec)?;
    Ok(result.map(|r| {
        format!(
            "{r}（命中：{:?}；文本：{shown}）",
            found.into_iter().collect::<Vec<_>>()
        )
    }))
}

/// 发出去的文件。index 默认 0；magic 是十六进制前缀（PNG = 89504e470d0a1a0a）。
fn file_check(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    let files = deps.platform.sent_files();
    let index = as_i64(spec, "index").unwrap_or(0);
    if index < 0 || files.is_empty() || index as usize >= files.len() {
        return Ok(Some(format!(
            "期望第 {index} 个 send_file，实际只发了 {} 个文件",
            files.len()
        )));
    }
    let f = &files[index as usize];
    if let Some(magic) = spec.get("magic") {
        let raw = text_of(magic);
        let want = hex_decode(&raw)
            .ok_or_else(|| CheckError(format!("check=file 的 magic 不是十六进制：{raw:?}")))?;
        let got = &f.data[..want.len().min(f.data.len())];
        if got != want.as_slice() {
            return Ok(Some(format!(
                "文件 {} 的前 {} 字节期望 {}，实际 {}",
                f.name,
                want.len(),
                hex_encode(&want),
                hex_encode(got)
            )));
        }
    }
    if let Some(suffix) = spec.get("name_suffix") {
        let suffix = text_of(suffix);
        if !f.name.ends_with(&suffix) {
            return Ok(Some(format!(
                "文件名期望以 {suffix:?} 结尾，实际 {:?}",
                f.name
            )));
        }
    }
    if let Some(mime) = spec.get("mime") {
        let mime = text_of(mime);
        if f.mime != mime {
            return Ok(Some(format!("文件 mime 期望 {mime:?}，实际 {:?}", f.mime)));
        }
    }
    if let Some(min_size) = as_i64(spec, "min_size")
        && (f.data.len() as i64) < min_size
    {
        return Ok(Some(format!(
            "文件 {} 期望至少 {min_size} 字节，实际 {}",
            f.name,
            f.data.len()
        )));
    }
    Ok(None)
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// 某个 Gateway 工具被真正执行了几次（本地 checklist_* 不走 Gateway，用 model_tools 数）。
fn gateway_calls(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    let name = need_str(spec, "name")?;
    compare(
        &format!("gateway.{name}"),
        deps.gateway.count(&name) as i64,
        spec,
    )
}

/// 某个工具返回的 content：ok / contains / not_contains / error_code。index 默认 -1。
fn gateway_result(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    let name = need_str(spec, "name")?;
    let results = deps.gateway.results_of(&name);
    if results.is_empty() {
        return Ok(Some(format!("{name} 一次都没被调用过，没有结果可看")));
    }
    let index = as_i64(spec, "index").unwrap_or(-1);
    let pos = if index < 0 {
        results.len() as i64 + index
    } else {
        index
    };
    if pos < 0 || pos as usize >= results.len() {
        return Ok(Some(format!(
            "{name} 期望第 {index} 次调用的结果，实际只有 {} 次",
            results.len()
        )));
    }
    let r = &results[pos as usize];
    if let Some(want) = spec.get("ok").and_then(Value::as_bool)
        && r.ok != want
    {
        let detail = match &r.error {
            Some(e) => e.message.clone(),
            None => r.content.chars().take(120).collect(),
        };
        return Ok(Some(format!(
            "{name} 期望 ok={want}，实际 ok={}（{detail}）",
            r.ok
        )));
    }
    if let Some(want) = spec.get("contains") {
        let want = text_of(want);
        if !r.content.contains(&want) {
            return Ok(Some(format!(
                "{name} 的结果里没有 {want:?}；实际：{:?}",
                r.content.chars().take(200).collect::<String>()
            )));
        }
    }
    if let Some(bad) = spec.get("not_contains") {
        let bad = text_of(bad);
        if r.content.contains(&bad) {
            return Ok(Some(format!(
                "{name} 的结果里不该出现 {bad:?}；实际：{:?}",
                r.content.chars().take(200).collect::<String>()
            )));
        }
    }
    if let Some(want) = spec.get("error_code") {
        let want = text_of(want);
        let got = r.error.as_ref().map(|e| e.code.as_str().to_string());
        if got.as_deref() != Some(want.as_str()) {
            return Ok(Some(format!("{name} 期望 error_code={want}，实际 {got:?}")));
        }
    }
    Ok(None)
}

/// 模型出过的 tool_call 名字计数（含本地 checklist_* 和 final）。
fn model_tools(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    let name = need_str(spec, "name")?;
    let emitted = deps.model.tool_names_emitted();
    compare(
        &format!("模型出牌 {name}"),
        emitted.iter().filter(|n| **n == name).count() as i64,
        spec,
    )
}

/// `ModelPort::chat` 被调了几次 —— 08_step_limit 用它验步数上限。
fn model_calls(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    compare("ModelPort.chat 次数", deps.model.call_count() as i64, spec)
}

/// 沙箱方法调用次数。07 用 `{method: release, min: 1}`。
fn sandbox_calls(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    let method = need_str(spec, "method")?;
    compare(
        &format!("sandbox.{method}"),
        deps.sandbox.calls().count(&method) as i64,
        spec,
    )
}

/// 会话/任务的账：sessions / tasks / task_sessions（任务落在几个不同会话上）/ seen_events。
fn store_check(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    let fields: [(&str, i64); 4] = [
        ("sessions", deps.store.session_count() as i64),
        ("tasks", deps.store.task_count() as i64),
        (
            "task_sessions",
            deps.store.distinct_session_ids_of_tasks().len() as i64,
        ),
        ("seen_events", deps.store.seen_count() as i64),
    ];
    let mut checked = false;
    for (field, actual) in fields {
        let prefix = format!("{field}_");
        let sub: Map<String, Value> = spec
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix(&prefix)
                    .map(|rest| (rest.to_string(), v.clone()))
            })
            .collect();
        if sub.is_empty() {
            continue;
        }
        checked = true;
        if let Some(reason) = compare(field, actual, &sub)? {
            return Ok(Some(reason));
        }
    }
    if !checked {
        return Err(CheckError(
            "check=store 至少要给 sessions_equals / tasks_equals / task_sessions_equals / \
             seen_events_equals 之一（也支持 _min / _max）"
                .to_string(),
        ));
    }
    Ok(None)
}

/// 任务终态。which: last（默认）/ first / any / all；status 是 TaskStatus 的值。
fn task_check(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    let tasks = deps.store.task_list();
    if tasks.is_empty() {
        let want = spec.get("status").cloned().unwrap_or(Value::Null);
        return Ok(Some(format!("期望有任务处于 {want}，实际一个任务都没建")));
    }
    let which = spec.get("which").map(text_of).unwrap_or("last".to_string());
    let want = need_str(spec, "status")?;
    let pool: Vec<&aite_contracts::Task> = match which.as_str() {
        "last" => tasks.last().into_iter().collect(),
        "first" => tasks.first().into_iter().collect(),
        "any" | "all" => tasks.iter().collect(),
        other => {
            return Err(CheckError(format!(
                "check=task 的 which 只能是 last / first / any / all，收到 {other:?}"
            )));
        }
    };
    let got: Vec<&str> = pool.iter().map(|t| t.status.as_str()).collect();
    let ok = if which == "all" {
        got.iter().all(|s| *s == want)
    } else {
        got.iter().any(|s| *s == want)
    };
    if !ok {
        return Ok(Some(format!(
            "任务状态（which={which}）期望 {want}，实际 {got:?}"
        )));
    }
    Ok(None)
}

/// 证据链：写了几条、链是否自洽。
fn evidence_check(deps: &Deps, spec: &Map<String, Value>) -> Outcome {
    if let Some(want) = spec.get("verified").and_then(Value::as_bool) {
        let bad: Vec<String> = deps
            .evidence
            .task_ids()
            .into_iter()
            .filter(|tid| !aite_contracts::EvidenceWriter::verify(&*deps.evidence, tid))
            .collect();
        if want && !bad.is_empty() {
            return Ok(Some(format!("这些任务的证据链验不过：{bad:?}")));
        }
    }
    if has_any(spec, &["equals", "min", "max"]) {
        return compare("证据条数", deps.evidence.total_events() as i64, spec);
    }
    if !spec.contains_key("verified") {
        return Err(CheckError(
            "check=evidence 至少要给 equals / min / max / verified".to_string(),
        ));
    }
    Ok(None)
}

/// 跑完全部断言，返回失败原因列表（空 = 全过）。未知 check / 畸形条目 / CheckError
/// 都是**失败行**，不往外抛。
pub fn run_checks(deps: &Deps, expect: &[Value]) -> Vec<String> {
    let mut failures = Vec::new();
    for (i, spec) in expect.iter().enumerate() {
        let Some(map) = spec.as_object() else {
            failures.push(format!("expect[{i}] 不是带 check 键的 mapping：{spec}"));
            continue;
        };
        if !map.contains_key("check") {
            failures.push(format!("expect[{i}] 不是带 check 键的 mapping：{spec}"));
            continue;
        }
        match run_check(deps, map) {
            Err(e) => failures.push(format!("expect[{i}] {}", e.0)),
            Ok(Some(reason)) => failures.push(reason),
            Ok(None) => {}
        }
    }
    failures
}
