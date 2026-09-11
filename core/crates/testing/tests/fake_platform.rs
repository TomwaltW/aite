//! FakePlatform 自测（移植自 `tests/e2e/test_t4_fake_platform.py`，14 条）：
//! 记账准不准、卡片是不是真的原地更新。
use std::collections::BTreeMap;
use std::sync::Arc;

use aite_contracts::{
    CardActionKind, CardStatus, ChecklistCard, ChecklistItemView, ChecklistState, DocumentContent,
    IngressError, NormalizedEvent, OutboundFile, OutboundText, PlatformPort, ReactionKind,
};
use aite_testing::{FakePlatform, HistoryBuilder, PNG_1X1, PNG_MAGIC, history_message};

fn make_card(status: CardStatus, items: &[&str]) -> ChecklistCard {
    ChecklistCard {
        task_id: "t1".to_string(),
        task_no: "#A1".to_string(),
        title: "跑对账".to_string(),
        initiator: "Alice".to_string(),
        started_at: "9:02".to_string(),
        status,
        items: items
            .iter()
            .enumerate()
            .map(|(i, t)| ChecklistItemView {
                id: format!("c{}", i + 1),
                text: t.to_string(),
                state: ChecklistState::Todo,
                note: None,
            })
            .collect(),
        footer: String::new(),
        actions: vec![CardActionKind::Stop],
    }
}

fn card() -> ChecklistCard {
    make_card(CardStatus::Working, &["拉数据"])
}

/// PlatformPort 的 10 个方法一个都不能少 —— Rust 里由 trait 实现保证（编译期），
/// 这条钉的是「实现的确实是契约那个 trait」与 capabilities 的 platform 取值。
#[tokio::test]
async fn implements_every_platform_port_method() {
    let p: Arc<dyn PlatformPort> = Arc::new(FakePlatform::new());
    assert_eq!(p.capabilities().platform, "fake");
    assert!(p.capabilities().supports_passive_listen);
}

#[tokio::test]
async fn send_text_is_recorded_with_params() {
    let p = FakePlatform::new();
    let msg = OutboundText {
        chat_id: "oc_1".into(),
        text: "你好".into(),
        reply_to: Some("om_1".into()),
        in_thread: true,
    };
    let res = p.send_text(&msg).await.unwrap();
    assert!(!res.message_id.is_empty());
    assert_eq!(p.count("send_text"), 1);
    let call = p.calls.last("send_text").unwrap();
    assert_eq!(call.arg_str("chat_id"), Some("oc_1"));
    assert_eq!(call.arg_str("reply_to"), Some("om_1"));
    assert_eq!(call.arg("in_thread").unwrap().as_bool(), Some(true));
    assert_eq!(call.arg_str("text"), Some("你好"));
    assert_eq!(p.texts(), vec!["你好".to_string()]);
}

/// 一张卡片、三次原地更新 —— 03 要的就是这个形状。
#[tokio::test]
async fn card_is_updated_in_place_not_resent() {
    let p = FakePlatform::new();
    let sent = p.send_card("oc_1", Some("om_1"), &card()).await.unwrap();
    let card_id = sent.card_id.clone().unwrap();
    for status in [
        CardStatus::Working,
        CardStatus::Working,
        CardStatus::Delivered,
    ] {
        p.update_card(&card_id, &make_card(status, &["拉数据"]))
            .await
            .unwrap();
    }
    assert_eq!(p.count("send_card"), 1);
    assert_eq!(p.update_count(), 3);
    assert_eq!(p.card_count(), 1); // 没有第二条卡片
    let snaps = p.card_snapshots(Some(&card_id));
    assert_eq!(snaps.len(), 4); // 初始 1 + 更新 3
    assert_eq!(snaps.last().unwrap().status, CardStatus::Delivered);
    // send_card 返回的 message_id 就是 card_id
    assert_eq!(sent.message_id, card_id);
}

/// 拿不存在的 card_id 更新 = 没有原地更新，必须当场报错。
#[tokio::test]
async fn update_card_with_unknown_id_errors() {
    let p = FakePlatform::new();
    let err = p.update_card("card-nope", &card()).await.unwrap_err();
    assert!(err.message.contains("不存在"), "{}", err.message);
}

/// 快照要冻住当时的样子，之后再改同一个对象不能污染历史。
#[tokio::test]
async fn card_snapshots_are_deep_copied() {
    let p = FakePlatform::new();
    let mut c = card();
    let sent = p.send_card("oc_1", None, &c).await.unwrap();
    c.items[0].state = ChecklistState::Done;
    let snaps = p.card_snapshots(sent.card_id.as_deref());
    assert_eq!(snaps[0].items[0].state, ChecklistState::Todo);
}

#[tokio::test]
async fn send_file_records_size_and_keeps_bytes() {
    let p = FakePlatform::new();
    p.send_file(&OutboundFile {
        chat_id: "oc_1".into(),
        reply_to: Some("om_1".into()),
        name: "out.png".into(),
        mime: "image/png".into(),
        data: PNG_1X1.clone(),
    })
    .await
    .unwrap();
    assert_eq!(p.count("send_file"), 1);
    assert_eq!(
        p.calls
            .last("send_file")
            .unwrap()
            .arg("size")
            .unwrap()
            .as_u64(),
        Some(PNG_1X1.len() as u64)
    );
    assert_eq!(&p.sent_files()[0].data[..8], &PNG_MAGIC);
}

/// 契约注释写死：过滤归 Gateway，adapter/平台不过滤。
#[tokio::test]
async fn read_history_does_not_filter_sender_kind() {
    let p = FakePlatform::new().with_history(vec![
        history_message("om_1", "人说的"),
        HistoryBuilder::new("om_2", "机器人播报")
            .sender_kind("bot")
            .build(),
    ]);
    let rows = p.read_history("oc_1", 50, None).await.unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r.sender_kind.as_str())
            .collect::<Vec<_>>(),
        vec!["human", "bot"]
    );
}

#[tokio::test]
async fn read_history_is_time_ordered_and_limited() {
    let history: Vec<_> = (0..5)
        .map(|i| history_message(&format!("om_{i}"), &format!("第{i}条")))
        .collect();
    let p = FakePlatform::new().with_history(history);
    let rows = p.read_history("oc_1", 2, None).await.unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r.message_id.clone())
            .collect::<Vec<_>>(),
        vec!["om_3".to_string(), "om_4".to_string()]
    );
}

#[tokio::test]
async fn read_history_filters_by_thread() {
    let p = FakePlatform::new().with_history(vec![
        history_message("om_1", "顶层"),
        HistoryBuilder::new("om_2", "话题里")
            .thread_id("om_1")
            .build(),
    ]);
    let rows = p.read_history("oc_1", 50, Some("om_1")).await.unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r.message_id.clone())
            .collect::<Vec<_>>(),
        vec!["om_2".to_string()]
    );
}

#[tokio::test]
async fn missing_document_and_file_report_with_hint() {
    let p = FakePlatform::new().with_documents(BTreeMap::from([(
        "k".to_string(),
        DocumentContent {
            title: "T".into(),
            text: String::new(),
            url: "u".into(),
        },
    )]));
    assert_eq!(p.read_document("k").await.unwrap().title, "T");
    let err = p.read_document("nope").await.unwrap_err();
    assert!(err.message.contains("没有这篇文档"), "{}", err.message);
    let err = p.download_file("om_1", "fk").await.unwrap_err();
    assert!(err.message.contains("没有这个附件"), "{}", err.message);
}

#[tokio::test]
async fn download_file_returns_bytes() {
    let p = FakePlatform::new().with_files(BTreeMap::from([(
        ("om_1".to_string(), "fk".to_string()),
        b"csv,data".to_vec(),
    )]));
    assert_eq!(p.download_file("om_1", "fk").await.unwrap(), b"csv,data");
}

/// 失败面要能演出来，且只演一次。
#[tokio::test]
async fn fail_next_injects_one_failure() {
    let p = FakePlatform::new();
    p.fail_next("send_text", "平台 500");
    let msg = OutboundText::new("oc_1", "x");
    let err = p.send_text(&msg).await.unwrap_err();
    assert!(err.message.contains("平台 500"));
    p.send_text(&msg).await.unwrap(); // 第二次恢复正常
}

#[tokio::test]
async fn outbound_count_covers_all_five_outbound_methods() {
    let p = FakePlatform::new();
    p.send_text(&OutboundText::new("oc_1", "a")).await.unwrap();
    let sent = p.send_card("oc_1", None, &card()).await.unwrap();
    p.update_card(sent.card_id.as_deref().unwrap(), &card())
        .await
        .unwrap();
    p.send_file(&OutboundFile {
        chat_id: "oc_1".into(),
        reply_to: None,
        name: "f".into(),
        mime: "text/plain".into(),
        data: b"x".to_vec(),
    })
    .await
    .unwrap();
    p.add_reaction("om_1", ReactionKind::Ack).await.unwrap();
    assert_eq!(p.outbound_count(), 5);
}

/// 没接上 on_event 就投事件 = 测试写错了，要当场说清楚。
#[tokio::test]
async fn emit_requires_start() {
    let p = Arc::new(FakePlatform::new());
    let ev = sample_event();
    let err = p.emit(&ev).await.unwrap_err();
    assert!(err.0.contains("还没有 start"), "{}", err.0);

    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = seen.clone();
    p.start(Arc::new(move |ev: NormalizedEvent| {
        let sink = sink.clone();
        Box::pin(async move {
            sink.lock().unwrap().push(ev.event_id);
            Ok::<(), IngressError>(())
        })
    }))
    .await
    .unwrap();
    assert!(p.started() && !p.stopped());
    p.emit(&ev).await.unwrap();
    assert_eq!(seen.lock().unwrap().as_slice(), ["e1".to_string()]);
    p.stop().await.unwrap();
    assert!(p.stopped());
}

/// 假 id 是**单一共享计数器**：msg / card / file 跨类型递增。
#[tokio::test]
async fn fake_ids_share_one_counter() {
    let p = FakePlatform::new();
    let m = p.send_text(&OutboundText::new("oc_1", "a")).await.unwrap();
    let c = p.send_card("oc_1", None, &card()).await.unwrap();
    let f = p
        .send_file(&OutboundFile {
            chat_id: "oc_1".into(),
            reply_to: None,
            name: "f".into(),
            mime: "text/plain".into(),
            data: b"x".to_vec(),
        })
        .await
        .unwrap();
    assert_eq!(m.message_id, "msg-1");
    assert_eq!(c.card_id.unwrap(), "card-2");
    assert_eq!(f.message_id, "file-3");
}

fn sample_event() -> NormalizedEvent {
    use aite_contracts::{Anchor, ChatType, EventKind, SenderKind};
    use chrono::{TimeZone, Utc};
    NormalizedEvent {
        event_id: "e1".into(),
        kind: EventKind::Message,
        platform: "fake".into(),
        tenant_id: "default".into(),
        workspace_id: "cli_fake_app".into(),
        chat_id: "oc_demo".into(),
        chat_type: ChatType::Group,
        sender_id: "ou_alice".into(),
        sender_kind: SenderKind::Human,
        sender_name: Some("Alice".into()),
        text: "你好".into(),
        raw_text: None,
        mentioned: true,
        anchor: Anchor {
            platform: "fake".into(),
            chat_id: "oc_demo".into(),
            message_id: "om_e1".into(),
            thread_id: None,
            task_no: None,
        },
        attachments: Vec::new(),
        card_action: None,
        occurred_at: Utc.with_ymd_and_hms(2026, 9, 9, 9, 0, 0).unwrap(),
        raw: serde_json::Map::new(),
    }
}
