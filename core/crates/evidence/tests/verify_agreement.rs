//! BB2 ①：把「两处 verify 口径一致」钉住。
//!
//! 校验证据链的代码在仓库里有**两份**：
//!
//! * `FileEvidenceWriter::verify`（`writer.rs` 的 `verify_sync`）—— 返回一个 bool，
//!   给 evals 的 `check=evidence verified` 和 worker 收尾用；
//! * `cli::load_timeline`（`cli.rs`）—— 给 `aite evidence show` 用，把问题逐条收起来。
//!
//! 两份的注释都写着「口径一致」，但**在此之前没有任何一条测试钉这句话**：
//! 谁改一处忘了另一处，都要等到真机排障时才发现 —— 那时候两个工具一个说绿一个说红，
//! 没人知道该信哪个。这个文件就是那条钉子。
//!
//! **它故意不判「谁对」，只判「两边一样」**：一条规则怎么写是别处的事，
//! 这里只保证两处不会各写各的。所以口径本身改了（比如 `created_at` 进链），
//! 这个文件一个字都不用动，它照样在钉 —— 钉的是一致性，不是某一版规则。
//!
//! 比的是**链上的判定**：`writer.verify()` 对 `timeline.issues.is_empty()`。
//! 不比 `Timeline::ok()` —— 那个还含 `manifest_problems`，而 `verify()` 按设计
//! 根本不读 manifest（`writer.rs` 的 doc 写着「只读 events.jsonl」）。
//! 拿 `ok()` 去比就是在拿两个本来就不同范围的东西较劲。
mod common;

use aite_contracts::{EvidenceKind, EvidenceWriter};
use aite_evidence::FileEvidenceWriter;
use aite_evidence::cli::load_timeline;
use common::{lines_of, p, rewrite_lines};
use serde_json::{Value, json};
use tempfile::TempDir;

const TASK: &str = "tsk_agree";

fn writer(tmp: &TempDir) -> FileEvidenceWriter {
    FileEvidenceWriter::new(tmp.path().join("evidence"))
}

/// 一条有内容的链：内联 payload 若干 + 一条超过 64KB 走 `payloads/{seq}.json` 的。
async fn build_chain(w: &FileEvidenceWriter) {
    w.append(
        TASK,
        EvidenceKind::TaskCreated,
        p(json!({"task_no": "#A1"})),
    )
    .await
    .unwrap();
    w.append(
        TASK,
        EvidenceKind::ModelCall,
        p(json!({"model": "scripted", "step": 1})),
    )
    .await
    .unwrap();
    w.append(
        TASK,
        EvidenceKind::ToolCall,
        p(json!({"name": "run_python", "blob": "x".repeat(70 * 1024)})),
    )
    .await
    .unwrap();
    w.append(
        TASK,
        EvidenceKind::Delivered,
        p(json!({"artifacts": 1, "missing": []})),
    )
    .await
    .unwrap();
}

/// events.jsonl 的每一行解析成 `Value`，交给 `mutate` 改完再原样写回。
fn rewrite(w: &FileEvidenceWriter, mutate: impl FnOnce(&mut Vec<Value>)) {
    let mut lines: Vec<Value> = lines_of(w, TASK)
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    mutate(&mut lines);
    let out: Vec<String> = lines
        .iter()
        .map(|v| serde_json::to_string(v).unwrap())
        .collect();
    rewrite_lines(w, TASK, &out);
}

/// 一格：名字 + 把链改坏的手法。手法可以什么都不做（干净那一格）。
struct Case {
    name: &'static str,
    damage: fn(&FileEvidenceWriter),
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "干净（一个字节都没动）",
            damage: |_| {},
        },
        Case {
            name: "改 payload 的内容",
            damage: |w| rewrite(w, |ls| ls[1]["payload"]["model"] = json!("别的模型")),
        },
        Case {
            name: "改 payload_hash",
            damage: |w| rewrite(w, |ls| ls[1]["payload_hash"] = json!("0".repeat(64))),
        },
        Case {
            name: "改 prev_hash",
            damage: |w| rewrite(w, |ls| ls[2]["prev_hash"] = json!("f".repeat(64))),
        },
        Case {
            name: "改 hash",
            damage: |w| rewrite(w, |ls| ls[2]["hash"] = json!("a".repeat(64))),
        },
        Case {
            name: "改 seq（跳号）",
            damage: |w| rewrite(w, |ls| ls[2]["seq"] = json!(99)),
        },
        Case {
            name: "中间混进别的任务的 task_id",
            damage: |w| rewrite(w, |ls| ls[1]["task_id"] = json!("tsk_别人")),
        },
        Case {
            name: "改 created_at（BB2 ① 的主角）",
            damage: |w| rewrite(w, |ls| ls[1]["created_at"] = json!("2030-01-01T00:00:00Z")),
        },
        Case {
            name: "删掉中间一行",
            damage: |w| {
                rewrite(w, |ls| {
                    ls.remove(1);
                })
            },
        },
        Case {
            name: "中间插一个空行",
            damage: |w| {
                let mut lines = lines_of(w, TASK);
                lines.insert(2, String::new());
                rewrite_lines(w, TASK, &lines);
            },
        },
        Case {
            name: "中间插一行不是 JSON 的字节",
            damage: |w| {
                let mut lines = lines_of(w, TASK);
                lines.insert(2, "{这不是 JSON".to_string());
                rewrite_lines(w, TASK, &lines);
            },
        },
        Case {
            name: "末尾留一段没写完的残行",
            damage: |w| {
                common::tear(w, TASK, "{\"task_id\":\"tsk_agree\",\"seq\":4,\"ki");
            },
        },
        Case {
            name: "外置 payload 文件被删掉",
            damage: |w| {
                // seq=2 那条超过 64KB，落在 payloads/2.json
                std::fs::remove_file(w.task_dir(TASK).join("payloads/2.json")).unwrap();
            },
        },
        Case {
            name: "events.jsonl 清空",
            damage: |w| std::fs::write(w.events_path(TASK), b"").unwrap(),
        },
        Case {
            name: "events.jsonl 不是合法 UTF-8",
            damage: |w| std::fs::write(w.events_path(TASK), [0xff, 0xfe, 0x0a]).unwrap(),
        },
    ]
}

/// 每一格都：新建一条链 → 按这一格的手法改坏 → 两处 verify 必须给同一个答案。
///
/// 一次跑完所有格再统一报，省得修一条跑一遍才看见下一条。
#[tokio::test]
async fn both_verifies_agree_on_every_kind_of_damage() {
    let mut disagreements: Vec<String> = Vec::new();

    for case in cases() {
        let tmp = TempDir::new().unwrap();
        let w = writer(&tmp);
        build_chain(&w).await;
        (case.damage)(&w);

        let by_writer = w.verify(TASK);
        let tl = load_timeline(&w.task_dir(TASK), 0.0, 0.0);
        let by_cli = tl.issues.is_empty();

        if by_writer != by_cli {
            let why: Vec<String> = tl
                .issues
                .iter()
                .map(|i| format!("行{} {}", i.line, i.problem))
                .collect();
            disagreements.push(format!(
                "「{}」：writer.verify={by_writer}，cli 的链上问题={}（{}）",
                case.name,
                tl.issues.len(),
                if why.is_empty() {
                    "无".to_string()
                } else {
                    why.join("；")
                }
            ));
        }
    }

    assert!(
        disagreements.is_empty(),
        "两处 verify 口径漂了 {} 格：\n  {}",
        disagreements.len(),
        disagreements.join("\n  ")
    );
}

/// 目录根本不在时，两处也要一致（这一格没法套上面那个模板：链都没建）。
#[test]
fn both_verifies_agree_when_the_task_dir_is_missing() {
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    let tl = load_timeline(&w.task_dir(TASK), 0.0, 0.0);
    assert_eq!(
        w.verify(TASK),
        tl.issues.is_empty(),
        "目录不在时两处答案不一样"
    );
    assert!(!w.verify(TASK), "目录不在还说链是好的");
}
