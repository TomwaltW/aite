//! 断言 DSL 的直接单元测试。
//!
//! 合流审核发现：14 类 check 里有 9 类**没有任何直接断言**，只能靠场景间接跑到；
//! 而并行期唯一能跑绿的 DemoPlane 只覆盖 4 个场景（01/02/09/10）——
//! 换句话说没接真 ControlPlane 时，这 9 类的代码路径在 CI 里一次都不会执行。
//! `checks::hex_decode` 那条「全角 magic 打穿整套评测」的 panic 正是这么漏到合流的。
//!
//! 这里绕开场景直接驱动替身造出状态，再调 `run_check`，让每一类的**通过**与
//! **失败**两条路都被走到。

use aite_contracts::{
    CardStatus, ChecklistCard, ChecklistItemView, ChecklistState, EvidenceKind, OutboundFile,
    OutboundText, PlatformPort, SessionStore, TaskStatus,
};
use aite_evals::{Deps, DepsOptions, Scenario, build_deps, run_check};
use serde_json::{Map, Value, json};

fn deps() -> Deps {
    build_deps(&Scenario::named("x"), &DepsOptions::default()).expect("造 deps")
}

fn spec(v: Value) -> Map<String, Value> {
    v.as_object().expect("spec 是对象").clone()
}

/// 跑一条断言：`Ok(None)` = 过，`Ok(Some(理由))` = 判失败，`Err` = 断言本身写错了。
fn passes(d: &Deps, v: Value) -> bool {
    matches!(run_check(d, &spec(v)), Ok(None))
}

fn fails_with(d: &Deps, v: Value) -> String {
    match run_check(d, &spec(v)) {
        Ok(Some(reason)) => reason,
        other => panic!("该判失败，实际 {other:?}"),
    }
}

fn card(task_no: &str, status: CardStatus, items: Vec<(&str, ChecklistState)>) -> ChecklistCard {
    ChecklistCard {
        task_id: "t1".into(),
        task_no: task_no.into(),
        title: "画图".into(),
        initiator: "张三".into(),
        started_at: "9:02".into(),
        status,
        items: items
            .into_iter()
            .map(|(id, state)| ChecklistItemView {
                id: id.into(),
                text: format!("第 {id} 步"),
                state,
                note: None,
            })
            .collect(),
        footer: String::new(),
        actions: vec![],
    }
}

fn task_at(id: &str, task_no: &str, status: TaskStatus) -> aite_contracts::Task {
    let now = chrono::Utc::now();
    aite_contracts::Task {
        id: id.into(),
        session_id: "s1".into(),
        task_no: task_no.into(),
        status,
        title: String::new(),
        checklist: Vec::new(),
        card_id: None,
        sandbox_id: None,
        session_token: "tok".into(),
        model: String::new(),
        steps: 0,
        tokens_in: 0,
        tokens_out: 0,
        cost: 0.0,
        max_steps: 40,
        max_wall_sec: 1200,
        result_summary: String::new(),
        evidence_root_hash: None,
        created_by: "ou".into(),
        created_at: now,
        updated_at: now,
    }
}

// --- cards：updates_min / final_status ---------------------------------------

#[tokio::test]
async fn cards_counts_distinct_updates_and_final_status() {
    let d = deps();
    let res = d
        .platform
        .send_card("oc", None, &card("#A1", CardStatus::Working, vec![]))
        .await
        .expect("发卡片");
    let id = res.card_id.clone().expect("card_id");
    for state in [ChecklistState::Doing, ChecklistState::Done] {
        d.platform
            .update_card(&id, &card("#A1", CardStatus::Working, vec![("c1", state)]))
            .await
            .expect("更新");
    }
    d.platform
        .update_card(&id, &card("#A1", CardStatus::Delivered, vec![]))
        .await
        .expect("收尾");

    assert!(passes(&d, json!({"check": "cards", "distinct_equals": 1})));
    assert!(passes(&d, json!({"check": "cards", "updates_min": 3})));
    assert!(passes(
        &d,
        json!({"check": "cards", "final_status": "delivered"})
    ));

    // 失败面：发了一张却期望两张；更新 3 次却期望 4 次；终态对不上
    assert!(fails_with(&d, json!({"check": "cards", "distinct_equals": 2})).contains('2'));
    assert!(!fails_with(&d, json!({"check": "cards", "updates_min": 4})).is_empty());
    let reason = fails_with(&d, json!({"check": "cards", "final_status": "failed"}));
    assert!(reason.contains("delivered"), "{reason}");
}

#[tokio::test]
async fn cards_without_any_card_reports_instead_of_passing_vacuously() {
    let d = deps();
    assert!(passes(&d, json!({"check": "cards", "distinct_equals": 0})));
    // 一张卡都没有时问终态，必须报失败而不是静默过
    assert!(!fails_with(&d, json!({"check": "cards", "final_status": "delivered"})).is_empty());
}

// --- file / magic -------------------------------------------------------------

#[tokio::test]
async fn file_checks_magic_name_mime_and_size() {
    let d = deps();
    d.platform
        .send_file(&OutboundFile {
            chat_id: "oc".into(),
            reply_to: None,
            name: "out.png".into(),
            mime: "image/png".into(),
            data: aite_testing::samples::png_bytes(1, 1, (255, 255, 255)),
        })
        .await
        .expect("发文件");

    assert!(passes(
        &d,
        json!({"check": "file", "magic": "89504e470d0a1a0a"})
    ));
    assert!(passes(
        &d,
        json!({"check": "file", "name_suffix": ".png", "mime": "image/png", "min_size": 10})
    ));
    // 负 index 从尾部数（与同文件的 gateway_result 一致，也与 Python files[-1] 一致）
    assert!(passes(
        &d,
        json!({"check": "file", "index": -1, "name_suffix": ".png"})
    ));

    let reason = fails_with(&d, json!({"check": "file", "magic": "ffd8ff"}));
    assert!(reason.contains("89504e"), "要把实际字节报出来：{reason}");
    assert!(!fails_with(&d, json!({"check": "file", "index": 3, "magic": "89"})).is_empty());
    assert!(!fails_with(&d, json!({"check": "file", "min_size": 999_999})).is_empty());
}

/// 回归：`magic` 写成全角时 `hex_decode` 曾按字节切片 panic，打穿整套评测。
#[tokio::test]
async fn a_non_ascii_magic_is_reported_not_panicked() {
    let d = deps();
    d.platform
        .send_file(&OutboundFile {
            chat_id: "oc".into(),
            reply_to: None,
            name: "a.bin".into(),
            mime: "application/octet-stream".into(),
            data: vec![1, 2, 3],
        })
        .await
        .expect("发文件");
    // 全角数字：6 字节、长度是偶数，能过「偶数长度」那一关
    let out = run_check(&d, &spec(json!({"check": "file", "magic": "８９"})));
    let err = out.expect_err("非十六进制的 magic 该报 CheckError（断言写错了），而不是 panic");
    assert!(err.0.contains("十六进制"), "{}", err.0);
}

// --- gateway_calls / gateway_result / model_tools / sandbox_calls --------------

#[tokio::test]
async fn gateway_and_sandbox_call_counts() {
    let d = deps();
    // 一次都没调过时：期望 0 该过，期望 >=1 该失败
    assert!(passes(
        &d,
        json!({"check": "gateway_calls", "name": "run_python", "equals": 0})
    ));
    let reason = fails_with(
        &d,
        json!({"check": "gateway_calls", "name": "run_python", "min": 1}),
    );
    assert!(reason.contains("run_python"), "{reason}");

    assert!(passes(
        &d,
        json!({"check": "sandbox_calls", "method": "release", "equals": 0})
    ));
    assert!(
        !fails_with(
            &d,
            json!({"check": "sandbox_calls", "method": "release", "min": 1})
        )
        .is_empty()
    );

    assert!(passes(
        &d,
        json!({"check": "model_tools", "name": "final", "equals": 0})
    ));
    assert!(passes(&d, json!({"check": "model_calls", "equals": 0})));
}

#[tokio::test]
async fn gateway_result_reports_when_never_called() {
    let d = deps();
    let reason = fails_with(
        &d,
        json!({"check": "gateway_result", "name": "run_python", "ok": true}),
    );
    assert!(reason.contains("一次都没"), "{reason}");
}

// --- evidence -----------------------------------------------------------------

#[tokio::test]
async fn evidence_counts_and_verifies_the_chain() {
    let d = deps();
    for kind in [
        EvidenceKind::TaskCreated,
        EvidenceKind::EventReceived,
        EvidenceKind::Delivered,
    ] {
        aite_contracts::EvidenceWriter::append(&*d.evidence, "t1", kind, Map::new())
            .await
            .expect("写证据");
    }
    assert!(passes(&d, json!({"check": "evidence", "equals": 3})));
    assert!(passes(&d, json!({"check": "evidence", "verified": true})));
    assert!(!fails_with(&d, json!({"check": "evidence", "equals": 5})).is_empty());
}

// --- task 的 which 四档 --------------------------------------------------------

#[tokio::test]
async fn task_which_first_any_all_and_last() {
    let d = deps();
    d.store.init().await.expect("init");
    let delivered = task_at("t1", "#A1", TaskStatus::Delivered);
    let failed = task_at("t2", "#A2", TaskStatus::Failed);
    d.store.create_task(&delivered).await.expect("建 t1");
    d.store.create_task(&failed).await.expect("建 t2");

    assert!(passes(
        &d,
        json!({"check": "task", "which": "first", "status": "delivered"})
    ));
    assert!(passes(
        &d,
        json!({"check": "task", "which": "last", "status": "failed"})
    ));
    assert!(passes(
        &d,
        json!({"check": "task", "which": "any", "status": "delivered"})
    ));
    assert!(
        !fails_with(
            &d,
            json!({"check": "task", "which": "all", "status": "delivered"})
        )
        .is_empty(),
        "两个任务状态不同，all 该失败"
    );
}

// --- distinct_matches 的去重语义 ------------------------------------------------

#[tokio::test]
async fn distinct_matches_counts_unique_hits_not_total() {
    let d = deps();
    d.platform
        .send_text(&OutboundText::new(
            "oc",
            "引用 [om_h1] [om_h3] [om_h1] [om_h5]",
        ))
        .await
        .expect("发文本");

    // 4 处命中、3 个不重复 —— 数的必须是后者
    assert!(passes(
        &d,
        json!({"check": "distinct_matches", "pattern": "om_h[0-9]+", "equals": 3})
    ));
    assert!(
        !fails_with(
            &d,
            json!({"check": "distinct_matches", "pattern": "om_h[0-9]+", "equals": 4})
        )
        .is_empty(),
        "数成总命中数就说明去重没生效"
    );
}

/// 正则写错要当场报（CheckError），不是静默判失败。
#[tokio::test]
async fn a_malformed_pattern_is_a_check_error() {
    let d = deps();
    let out = run_check(
        &d,
        &spec(json!({"check": "distinct_matches", "pattern": "(ab", "equals": 1})),
    );
    assert!(out.is_err(), "括号没闭合该报 CheckError，实际 {out:?}");
}

// --- 比较子本身 ---------------------------------------------------------------

/// 回归：非整数比较子曾被**静默跳过** —— 断言凭空消失，比写错更危险。
#[tokio::test]
async fn a_non_integer_comparator_is_an_error_not_a_silent_pass() {
    let d = deps();
    for bad in [json!("1"), json!(1.5), json!(true), json!(null)] {
        let out = run_check(
            &d,
            &spec(json!({"check": "model_calls", "equals": bad.clone()})),
        );
        assert!(out.is_err(), "equals: {bad} 该报错，实际 {out:?}");
    }
    // 一个比较子都不给也要报错（断言写漏了）
    assert!(run_check(&d, &spec(json!({"check": "model_calls"}))).is_err());
}
