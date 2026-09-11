//! EvidenceWriter 的落盘实现（对应旧 `aite/evidence/writer.py`）。
//!
//! 目录形状（§3.1 末尾写死）：
//!
//! ```text
//! {evidence_dir}/{task_id}/events.jsonl        一行一个 EvidenceEvent
//! {evidence_dir}/{task_id}/manifest.json       {task_id, session_id, task_no, created_by,
//!                                               model, contract_version, root_hash, event_count}
//! {evidence_dir}/{task_id}/payloads/{seq}.json 仅当单条 payload > 64KB
//! ```
//!
//! 链式 hash 全部走契约里的 `payload_hash_of` / `chain_hash`，本文件不自己算 sha256，
//! 免得哪天两处实现漂开。
//!
//! **崩溃残行（T21）**：进程在写到一半没了、或者磁盘写满只落了一半，`events.jsonl`
//! 末尾会留下一段没有换行符收尾的字节。写入侧（`append` / `finalize`）进门先走一次
//! `heal_torn_tail` 把它收拾掉，否则 §3.3 要求的收尾链会自锁 —— 「任何未捕获异常 →
//! task failed + 回帖 + evidence failed 事件」里，写那条 failed 证据本身就是炸的那个操作，
//! 越出事越写不进去。
//!
//! 口径就一条，**换行符是记录终止符**：
//!
//! * 文件以换行符收尾 → 每条记录都完整落盘过，末尾没有残行；此时任何解析不了的行
//!   都是**中间被改坏**的，一律照旧报错 / `verify()` 报 false，不许在这里「修复」。
//! * 文件不以换行符收尾 → 末段没有终止符。它要么是没写完的半条（丢弃），要么是整条
//!   写完了只差那个换行符（补上，一个字节都不丢）。两者靠「这段能不能解析成
//!   EvidenceEvent 并接得住前面的链」分开 —— 也就是 `verify()` 本来就用的那把尺子。
//!
//! 由此得到的性质：收拾只丢弃 `verify()` 本来就会拒绝的字节，一条它认可的记录都不动 ——
//! 所以收拾只可能把 `verify()` 从 false 修回 true，绝不会反过来。`verify()` 自己**不**
//! 收拾，它看到的永远是文件此刻的真实样子：残行在它那里照旧是 false。
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use aite_contracts::{
    CONTRACT_VERSION, EvidenceError, EvidenceEvent, EvidenceKind, EvidenceWriter, GENESIS,
    canonical_json, chain_hash, payload_hash_of,
};
use async_trait::async_trait;
use chrono::{Timelike, Utc};
use serde_json::{Map, Value};

/// 超过这个大小的 payload 不内联，落到 `payloads/{seq}.json`（§3.1 `EvidenceEvent.payload_ref`）。
pub const INLINE_PAYLOAD_MAX_BYTES: usize = 64 * 1024;

pub const EVENTS_FILE: &str = "events.jsonl";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const PAYLOAD_DIR: &str = "payloads";

/// 残行原文进 WARNING 日志时截到这么长（字符数）。截掉的那半行不另存文件
/// （它从来没有效过），日志里这段前缀加上字节数就是它留下的全部痕迹。
pub const TORN_TAIL_LOG_CLIP: usize = 200;

/// 计数器名（§3.3 硬约束 3：计数器名逐字不变）。
pub const COUNTER_TORN_TAIL_KEPT: &str = "evidence.torn_tail_kept";
pub const COUNTER_TORN_TAIL_DROPPED: &str = "evidence.torn_tail_dropped";

/// manifest 的字段就是 §3.1 写死的这一组，不多不少。
const MANIFEST_DESCRIPTIVE_FIELDS: [&str; 4] = ["session_id", "task_no", "created_by", "model"];

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 一行是不是「空行」（Python 的 `line.strip()` 为空）。
fn is_blank(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| b.is_ascii_whitespace())
}

/// 现在几点，截到微秒。
///
/// Python 的 `datetime` 只有微秒精度，旧证据目录里的 `created_at` 也一律是 6 位小数。
/// chrono 的 RFC3339 序列化按实际值挑 0/3/6/9 位，在纳秒分辨率的时钟上会打出 9 位 ——
/// 那种行 Python 侧的 `verify` 不一定认。这里截一刀，两边落盘形状就是同一个。
fn now_micros() -> chrono::DateTime<Utc> {
    let now = Utc::now();
    now.with_nanosecond(now.nanosecond() / 1_000 * 1_000)
        .unwrap_or(now)
}

struct Inner {
    root: PathBuf,
    /// task_id -> (下一个 seq, 上一条 hash)。只是省掉每次 append 重读整个 jsonl
    /// （40 步的任务有一百多条事件，不缓存就是 O(n²) 次解析）。
    /// 首次接触某个任务时仍从文件恢复，所以进程重启、换实例都接得上。
    tip: Mutex<HashMap<String, (u64, String)>>,
    /// 崩溃恢复留痕（§3.3 排障用）。静默截掉一条记录跟静默坏链一样难查。
    counters: Mutex<BTreeMap<String, u64>>,
    /// 读过几次 events.jsonl 全文。热路径（健康文件上的 append）必须是 0 ——
    /// 测试拿它钉住「一次 stat + 读末字节」的成本，别退化成每次读一遍再判断。
    whole_file_reads: AtomicU64,
}

impl Inner {
    fn task_dir(&self, task_id: &str) -> PathBuf {
        self.root.join(task_id)
    }

    fn events_path(&self, task_id: &str) -> PathBuf {
        self.task_dir(task_id).join(EVENTS_FILE)
    }

    fn manifest_path(&self, task_id: &str) -> PathBuf {
        self.task_dir(task_id).join(MANIFEST_FILE)
    }

    fn bump(&self, key: &str) {
        *lock(&self.counters).entry(key.to_string()).or_insert(0) += 1;
    }

    /// 读全文。唯一一处读整个 events.jsonl 的地方，顺手计数。
    fn read_whole(&self, path: &Path) -> Result<Vec<u8>, EvidenceError> {
        self.whole_file_reads.fetch_add(1, Ordering::Relaxed);
        Ok(fs::read(path)?)
    }

    // ---- 崩溃残行 --------------------------------------------------------

    /// 收拾 `events.jsonl` 末尾那段没有换行符收尾的字节（口径见模块 doc）。
    ///
    /// 热路径的代价是一次 stat 加读最后 1 字节，与文件长度无关 —— 每条证据都会走这里，
    /// 不能全文读一遍。只有真发现残行时才升级成读全文。
    fn heal_torn_tail(&self, task_id: &str) -> Result<(), EvidenceError> {
        let path = self.events_path(task_id);
        if !path.exists() {
            return Ok(());
        }
        let size = fs::metadata(&path)?.len();
        if size == 0 {
            return Ok(()); // 空文件没有残行
        }
        {
            let mut fh = File::open(&path)?;
            fh.seek(SeekFrom::End(-1))?;
            let mut last = [0u8; 1];
            fh.read_exact(&mut last)?;
            if last[0] == b'\n' {
                return Ok(()); // 每条记录都有终止符，正常路径到此为止
            }
        }

        // 按字节切，不 decode 全文：残行可能截在一个 UTF-8 多字节字符中间，
        // 先 decode 会炸在非法 UTF-8 上，那就等于没修。
        let raw = self.read_whole(&path)?;
        let cut = raw.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        let tail = &raw[cut..];
        let shown: String = String::from_utf8_lossy(tail)
            .chars()
            .take(TORN_TAIL_LOG_CLIP)
            .collect();

        if self.tail_is_a_whole_record(task_id, &raw[..cut], tail) {
            // 整条写完了，只差那个换行符：补上，一个字节都不丢（verify 本来就认它）
            let mut fh = OpenOptions::new().append(true).open(&path)?;
            fh.write_all(b"\n")?;
            self.bump(COUNTER_TORN_TAIL_KEPT);
            tracing::warn!(
                "证据链 {} 末尾少了换行符，末段是完整的一条，已补齐（{} 字节）：{}",
                path.display(),
                tail.len(),
                shown
            );
        } else {
            // 真残行：从来没写完，也就从来没有效过。截到最后一个完整记录的换行处。
            let fh = OpenOptions::new().write(true).open(&path)?;
            fh.set_len(cut as u64)?;
            self.bump(COUNTER_TORN_TAIL_DROPPED);
            tracing::warn!(
                "证据链 {} 末尾有 {} 字节没写完，已截掉 —— 上一个进程崩在 write 中途，\
                 或者磁盘写满了。残行原文：{}",
                path.display(),
                tail.len(),
                shown
            );
        }
        lock(&self.tip).remove(task_id); // 缓存跟文件对不上了，下一次重算
        Ok(())
    }

    /// 末段虽然没有换行符收尾，但它其实是完整的一条吗？
    ///
    /// 判据用的就是 `verify()` 那把尺子（解析得了、task_id 对、seq 接得上、hash 链闭合），
    /// 所以收拾前后 `verify()` 的答案不会变。只解析 head 的最后一行，不重算整条链 ——
    /// head 中间要是有被改坏的行，那是篡改，轮不到这里处理。
    fn tail_is_a_whole_record(&self, task_id: &str, head: &[u8], tail: &[u8]) -> bool {
        let Ok(ev) = serde_json::from_slice::<EvidenceEvent>(tail) else {
            return false;
        };
        if ev.task_id != task_id {
            return false;
        }
        let lines: Vec<&[u8]> = head
            .split(|b| *b == b'\n')
            .filter(|l| !is_blank(l))
            .collect();
        if ev.seq != lines.len() as u64 {
            return false;
        }
        let prev_hash = match lines.last() {
            None => GENESIS.to_string(),
            Some(last) => match serde_json::from_slice::<EvidenceEvent>(last) {
                Ok(prev) => prev.hash,
                Err(_) => return false,
            },
        };
        ev.prev_hash == prev_hash && ev.hash == chain_hash(&prev_hash, &ev.payload_hash)
    }

    // ---- 内部 ------------------------------------------------------------

    fn chain_tip(&self, task_id: &str) -> Result<(u64, String), EvidenceError> {
        if let Some(cached) = lock(&self.tip).get(task_id) {
            return Ok(cached.clone());
        }
        let events = self.read_events(task_id)?;
        let tip = match events.last() {
            None => (0, GENESIS.to_string()),
            Some(last) => (last.seq + 1, last.hash.clone()),
        };
        lock(&self.tip).insert(task_id.to_string(), tip.clone());
        Ok(tip)
    }

    /// 读全文。行解析不了就报错 —— 调用方都先过了 `heal_torn_tail`，所以走到这里
    /// 还坏的行只可能是中间被改坏的，不该被吞掉。
    fn read_events(&self, task_id: &str) -> Result<Vec<EvidenceEvent>, EvidenceError> {
        let path = self.events_path(task_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let raw = self.read_whole(&path)?;
        let mut out = Vec::new();
        for (i, line) in raw.split(|b| *b == b'\n').enumerate() {
            if is_blank(line) {
                continue;
            }
            match serde_json::from_slice::<EvidenceEvent>(line) {
                Ok(ev) => out.push(ev),
                Err(e) => {
                    return Err(EvidenceError::Corrupt {
                        task_id: task_id.to_string(),
                        detail: format!("第 {} 行不是合法的 EvidenceEvent：{e}", i + 1),
                    });
                }
            }
        }
        Ok(out)
    }

    fn resolve_payload(&self, task_id: &str, ev: &EvidenceEvent) -> Option<Map<String, Value>> {
        if let Some(p) = &ev.payload {
            return Some(p.clone());
        }
        let rel = ev.payload_ref.as_ref()?;
        let path = self.task_dir(task_id).join(rel);
        if !path.exists() {
            return None;
        }
        let text = fs::read_to_string(&path).ok()?;
        match serde_json::from_str::<Value>(&text).ok()? {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    // ---- 写 --------------------------------------------------------------

    fn append_sync(
        &self,
        task_id: &str,
        kind: EvidenceKind,
        payload: Map<String, Value>,
    ) -> Result<EvidenceEvent, EvidenceError> {
        // 上一条命可能写到一半。这一步必须在算 tip 之前，也必须真去看文件 ——
        // 磁盘满那一路 tip 还是热的、自以为写成功了，只有文件知道真相。
        self.heal_torn_tail(task_id)?;
        let (seq, prev_hash) = self.chain_tip(task_id)?;
        let body = canonical_json(&payload);
        let p_hash = payload_hash_of(&payload);

        let mut payload_ref: Option<String> = None;
        let mut inline: Option<Map<String, Value>> = Some(payload);
        if body.len() > INLINE_PAYLOAD_MAX_BYTES {
            let rel = format!("{PAYLOAD_DIR}/{seq}.json");
            let out = self.task_dir(task_id).join(&rel);
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&out, body.as_bytes())?;
            payload_ref = Some(rel);
            inline = None; // hash 仍按原 payload 算
        }

        let ev = EvidenceEvent {
            task_id: task_id.to_string(),
            seq,
            kind,
            payload_hash: p_hash.clone(),
            payload_ref,
            payload: inline,
            prev_hash: prev_hash.clone(),
            hash: chain_hash(&prev_hash, &p_hash),
            created_at: now_micros(),
        };

        let path = self.events_path(task_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut line = serde_json::to_string(&ev)?;
        line.push('\n');
        let mut fh = OpenOptions::new().create(true).append(true).open(&path)?;
        fh.write_all(line.as_bytes())?; // append 模式一次写完（无 fsync，同 Python 版）

        lock(&self.tip).insert(task_id.to_string(), (ev.seq + 1, ev.hash.clone()));
        Ok(ev)
    }

    fn finalize_sync(
        &self,
        task_id: &str,
        manifest_extra: Map<String, Value>,
    ) -> Result<String, EvidenceError> {
        // finalize 也是写入侧入口，崩溃后最先被调到的往往就是它。收拾之后
        // event_count 少掉的那一条正是没写完的半行 —— 它从来不是一条事件。
        self.heal_torn_tail(task_id)?;
        let events = self.read_events(task_id)?;
        let root_hash = events
            .last()
            .map_or_else(|| GENESIS.to_string(), |e| e.hash.clone());

        // BTreeMap 保证键序，不看 serde_json 的 Map 是什么实现（对齐 Python 的 sort_keys）
        let mut manifest: BTreeMap<String, Value> = BTreeMap::new();
        manifest.insert("task_id".into(), Value::String(task_id.to_string()));
        for field in MANIFEST_DESCRIPTIVE_FIELDS {
            // 缺省字段一律空串
            let v = manifest_extra
                .get(field)
                .cloned()
                .unwrap_or_else(|| Value::String(String::new()));
            manifest.insert(field.to_string(), v);
        }
        manifest.insert(
            "contract_version".into(),
            Value::String(CONTRACT_VERSION.to_string()),
        );
        manifest.insert("root_hash".into(), Value::String(root_hash.clone()));
        manifest.insert("event_count".into(), Value::from(events.len()));

        let path = self.manifest_path(task_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = serde_json::to_string_pretty(&manifest)?;
        text.push('\n');
        fs::write(&path, text.as_bytes())?;
        Ok(root_hash)
    }

    // ---- 校验 ------------------------------------------------------------

    /// 重算整条链：seq 连续、payload 与 payload_hash 对得上、hash 链闭合。
    ///
    /// **只读**：不做任何崩溃恢复。残行、被改坏的中间行，在这里一律是 false ——
    /// 它回答的是「文件此刻可不可信」，不是「收拾一下还能不能救」。
    fn verify_sync(&self, task_id: &str) -> bool {
        let path = self.events_path(task_id);
        if !path.exists() {
            return false;
        }
        let Ok(raw) = self.read_whole(&path) else {
            return false;
        };
        let Ok(text) = String::from_utf8(raw) else {
            return false;
        };
        let mut prev = GENESIS.to_string();
        for (i, line) in text.lines().enumerate() {
            // 空行 continue —— 但 i 已经消耗掉了，后面的 seq 就对不上：
            // 「events.jsonl 不许有空行」这条就是这么钉住的
            if line.trim().is_empty() {
                continue;
            }
            let Ok(ev) = serde_json::from_str::<EvidenceEvent>(line) else {
                return false;
            };
            if ev.task_id != task_id || ev.seq != i as u64 {
                return false;
            }
            let Some(payload) = self.resolve_payload(task_id, &ev) else {
                return false;
            };
            if payload_hash_of(&payload) != ev.payload_hash {
                return false;
            }
            if ev.prev_hash != prev || ev.hash != chain_hash(&prev, &ev.payload_hash) {
                return false;
            }
            prev = ev.hash;
        }
        true
    }
}

/// EvidenceWriter（对应 Python `FileEvidenceWriter`）。
pub struct FileEvidenceWriter {
    inner: Arc<Inner>,
    /// `append` 与 `finalize` 串行：链尾只有一个。`verify` 不加锁（只读）。
    write_lock: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for FileEvidenceWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileEvidenceWriter")
            .field("root", &self.inner.root)
            .finish()
    }
}

impl FileEvidenceWriter {
    pub fn new(evidence_dir: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Inner {
                root: evidence_dir.into(),
                tip: Mutex::new(HashMap::new()),
                counters: Mutex::new(BTreeMap::new()),
                whole_file_reads: AtomicU64::new(0),
            }),
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// 证据根目录。
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    pub fn events_path(&self, task_id: &str) -> PathBuf {
        self.inner.events_path(task_id)
    }

    pub fn manifest_path(&self, task_id: &str) -> PathBuf {
        self.inner.manifest_path(task_id)
    }

    /// 崩溃恢复留痕的快照。
    pub fn counters(&self) -> BTreeMap<String, u64> {
        lock(&self.inner.counters).clone()
    }

    pub fn counter(&self, name: &str) -> u64 {
        lock(&self.inner.counters).get(name).copied().unwrap_or(0)
    }

    /// events.jsonl 被整份读过几次。健康文件上的 append 必须一次都不读。
    pub fn whole_file_reads(&self) -> u64 {
        self.inner.whole_file_reads.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl EvidenceWriter for FileEvidenceWriter {
    async fn append(
        &self,
        task_id: &str,
        kind: EvidenceKind,
        payload: Map<String, Value>,
    ) -> Result<EvidenceEvent, EvidenceError> {
        let _guard = self.write_lock.lock().await;
        let inner = Arc::clone(&self.inner);
        let task_id = task_id.to_string();
        // 文件 IO 是阻塞的，不在 async 上下文里直接做（spec §7.6）
        tokio::task::spawn_blocking(move || inner.append_sync(&task_id, kind, payload))
            .await
            .map_err(|e| {
                EvidenceError::Io(std::io::Error::other(format!("spawn_blocking 失败：{e}")))
            })?
    }

    async fn finalize(
        &self,
        task_id: &str,
        manifest_extra: Map<String, Value>,
    ) -> Result<String, EvidenceError> {
        let _guard = self.write_lock.lock().await;
        let inner = Arc::clone(&self.inner);
        let task_id = task_id.to_string();
        tokio::task::spawn_blocking(move || inner.finalize_sync(&task_id, manifest_extra))
            .await
            .map_err(|e| {
                EvidenceError::Io(std::io::Error::other(format!("spawn_blocking 失败：{e}")))
            })?
    }

    fn verify(&self, task_id: &str) -> bool {
        self.inner.verify_sync(task_id)
    }

    fn task_dir(&self, task_id: &str) -> PathBuf {
        self.inner.task_dir(task_id)
    }
}
