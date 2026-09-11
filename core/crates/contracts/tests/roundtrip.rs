//! 所有 §3.1 模型的 JSON round-trip，外加几处必须与 Python 版一致的 JSON 细节。
use aite_contracts::*;
use chrono::{TimeZone, Utc};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};

fn rt<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(v: &T) -> Value {
    let text = serde_json::to_string(v).unwrap();
    let back: T = serde_json::from_str(&text).unwrap();
    assert_eq!(&back, v);
    serde_json::from_str(&text).unwrap()
}

fn anchor() -> Anchor {
    Anchor {
        platform: "feishu".into(),
        chat_id: "oc_1".into(),
        message_id: "om_1".into(),
        thread_id: Some("om_root".into()),
        task_no: None,
    }
}

fn event() -> NormalizedEvent {
    NormalizedEvent {
        event_id: "e1".into(),
        kind: EventKind::Message,
        platform: "feishu".into(),
        tenant_id: "default".into(),
        workspace_id: "cli_x".into(),
        chat_id: "oc_1".into(),
        chat_type: ChatType::Group,
        sender_id: "ou_1".into(),
        sender_kind: SenderKind::Human,
        sender_name: Some("张三".into()),
        text: "你好".into(),
        raw_text: Some("@_user_1 你好".into()),
        mentioned: true,
        anchor: anchor(),
        attachments: vec![Attachment {
            kind: AttachmentKind::File,
            file_key: "file_v2_1".into(),
            message_id: "om_1".into(),
            name: Some("a.csv".into()),
            size: Some(20480),
            mime: None,
        }],
        card_action: Some(CardAction {
            card_id: "om_card".into(),
            action: CardActionKind::Stop,
            task_id: Some("t1".into()),
            value: json!({"action": "stop", "task_id": "t1"})
                .as_object()
                .unwrap()
                .clone(),
        }),
        occurred_at: Utc.with_ymd_and_hms(2026, 9, 9, 1, 7, 0).unwrap(),
        raw: json!({"header": {"event_id": "e1"}})
            .as_object()
            .unwrap()
            .clone(),
    }
}

#[test]
fn normalized_event_round_trips_with_python_field_values() {
    let v = rt(&event());
    assert_eq!(v["kind"], "message");
    assert_eq!(v["chat_type"], "group");
    assert_eq!(v["sender_kind"], "human");
    assert_eq!(v["attachments"][0]["kind"], "file");
    assert_eq!(v["card_action"]["action"], "stop");
    assert_eq!(v["anchor"]["task_no"], Value::Null);
    let t = v["occurred_at"].as_str().unwrap();
    assert!(t.starts_with("2026-09-09T01:07:00"), "{t}");
}

#[test]
fn python_style_timestamps_parse() {
    for ts in [
        "2026-09-09T01:07:00Z",
        "2026-09-09T01:07:00+00:00",
        "2026-09-09T01:07:00.123456Z",
    ] {
        let mut v = serde_json::to_value(event()).unwrap();
        v["occurred_at"] = json!(ts);
        let ev: NormalizedEvent = serde_json::from_value(v).unwrap();
        assert_eq!(
            ev.occurred_at.timestamp(),
            Utc.with_ymd_and_hms(2026, 9, 9, 1, 7, 0)
                .unwrap()
                .timestamp(),
            "{ts}"
        );
    }
}

#[test]
fn optional_fields_default_when_missing() {
    let minimal = json!({
        "event_id": "e", "kind": "card_action", "platform": "fake", "workspace_id": "w",
        "chat_id": "c", "chat_type": "p2p", "sender_id": "s", "sender_kind": "bot",
        "text": "", "mentioned": false, "anchor": {"platform": "fake", "chat_id": "c", "message_id": "m"},
        "occurred_at": "2026-09-09T01:07:00Z"
    });
    let ev: NormalizedEvent = serde_json::from_value(minimal).unwrap();
    assert_eq!(ev.tenant_id, "default");
    assert!(ev.attachments.is_empty() && ev.card_action.is_none() && ev.raw.is_empty());
    assert_eq!(ev.anchor.thread_id, None);
}

#[test]
fn outbound_models() {
    let text = rt(&OutboundText::new("oc_1", "**hi**"));
    assert_eq!(text["in_thread"], true);
    let parsed: OutboundText =
        serde_json::from_value(json!({"chat_id": "c", "text": "t"})).unwrap();
    assert!(parsed.in_thread, "in_thread 默认 true");

    let card = ChecklistCard {
        task_id: "t".into(),
        task_no: "#A1".into(),
        title: "画图".into(),
        initiator: "张三".into(),
        started_at: "9:02".into(),
        status: CardStatus::Working,
        items: vec![ChecklistItemView {
            id: "c1".into(),
            text: "读文件".into(),
            state: ChecklistState::Doing,
            note: None,
        }],
        footer: "已用 1 步 · ¥0.00".into(),
        actions: vec![CardActionKind::Stop, CardActionKind::Evidence],
    };
    let v = rt(&card);
    assert_eq!(v["status"], "working");
    assert_eq!(v["items"][0]["state"], "doing");
    assert_eq!(v["actions"], json!(["stop", "evidence"]));
    let parsed: ChecklistCard = serde_json::from_value(json!({
        "task_id": "t", "task_no": "#A1", "title": "x", "initiator": "y", "started_at": "9:02", "status": "failed"
    }))
    .unwrap();
    assert_eq!(
        parsed.actions,
        vec![CardActionKind::Stop],
        "actions 默认 [stop]"
    );

    rt(&OutboundFile {
        chat_id: "c".into(),
        reply_to: None,
        name: "a.png".into(),
        mime: "image/png".into(),
        data: vec![0x89, 0x50, 0x4e, 0x47],
    });
    rt(&SendResult {
        message_id: "om".into(),
        card_id: Some("om".into()),
    });
    assert_eq!(serde_json::to_value(ReactionKind::Ack).unwrap(), "ack");
    rt(&HistoryMessage {
        message_id: "m".into(),
        sender_id: "s".into(),
        sender_kind: "human".into(),
        sender_name: None,
        text: "t".into(),
        thread_id: None,
        created_at: Utc::now(),
    });
    rt(&DocumentContent {
        title: "T".into(),
        text: "# T".into(),
        url: "https://x".into(),
    });
}

#[test]
fn session_and_task() {
    let now = Utc::now();
    let s = Session {
        id: "s1".into(),
        tenant_id: "default".into(),
        workspace_id: "w".into(),
        chat_id: "c".into(),
        kind: SessionKind::Task,
        anchor: anchor(),
        status: SessionStatus::Active,
        created_by: "ou_1".into(),
        config_snapshot: json!({"initiator_name": "张三"})
            .as_object()
            .unwrap()
            .clone(),
        created_at: now,
        last_active_at: now,
        archived_at: None,
    };
    let v = rt(&s);
    assert_eq!(v["kind"], "task");
    assert_eq!(v["status"], "active");

    let t = Task {
        id: "t1".into(),
        session_id: "s1".into(),
        task_no: "#A1".into(),
        status: TaskStatus::Working,
        title: "x".into(),
        checklist: vec![ChecklistItem {
            id: "c1".into(),
            text: "a".into(),
            state: ChecklistState::Todo,
            note: None,
        }],
        card_id: None,
        sandbox_id: None,
        session_token: "ab".repeat(16),
        model: "m".into(),
        steps: 3,
        tokens_in: 10,
        tokens_out: 5,
        cost: 0.5,
        max_steps: 40,
        max_wall_sec: 1200,
        result_summary: String::new(),
        evidence_root_hash: None,
        created_by: "ou_1".into(),
        created_at: now,
        updated_at: now,
    };
    let v = rt(&t);
    assert_eq!(v["status"], "working");
    assert_eq!(v["checklist"][0]["state"], "todo");
    let parsed: Task = serde_json::from_value(json!({
        "id": "t", "session_id": "s", "task_no": "#A1", "session_token": "x", "created_by": "u",
        "created_at": "2026-09-09T01:07:00Z", "updated_at": "2026-09-09T01:07:00Z"
    }))
    .unwrap();
    assert_eq!(
        (parsed.status, parsed.max_steps, parsed.max_wall_sec),
        (TaskStatus::Created, 40, 1200)
    );
    assert!(
        TaskStatus::Working.is_active()
            && !TaskStatus::Answering.is_active()
            && TaskStatus::Failed.is_terminal()
    );

    rt(&Turn {
        session_id: "s".into(),
        seq: 0,
        role: TurnRole::SystemNote,
        platform_user_id: None,
        content: "[用户修改了消息] 新内容：x".into(),
        attachments: vec![],
        created_at: now,
    });
    assert_eq!(
        serde_json::to_value(TurnRole::SystemNote).unwrap(),
        "system_note"
    );
}

#[test]
fn protocol_gateway_sandbox_evidence_models() {
    rt(&Message {
        role: Role::Assistant,
        content: String::new(),
        tool_calls: Some(vec![ToolCallRequest {
            call_id: "c".into(),
            name: "final".into(),
            arguments: json!({"reply": "ok"}).as_object().unwrap().clone(),
        }]),
        tool_call_id: None,
        name: None,
    });
    let turn = rt(&ModelTurn {
        message: Message::text(Role::Assistant, "hi"),
        usage: Usage {
            input_tokens: 1,
            output_tokens: 2,
            cached_tokens: 0,
        },
        finish_reason: "stop".into(),
        raw: Map::new(),
    });
    assert_eq!(turn["message"]["role"], "assistant");
    let parsed: ModelTurn =
        serde_json::from_value(json!({"message": {"role": "assistant", "content": "x"}})).unwrap();
    assert_eq!(parsed.finish_reason, "stop");

    let r = rt(&ToolResult {
        call_id: "c".into(),
        name: "run_python".into(),
        ok: false,
        content: "[sandbox] x".into(),
        data: None,
        error: Some(ToolError {
            code: ToolErrorCode::Sandbox,
            message: "x".into(),
        }),
        duration_ms: 3,
        artifacts: vec![ArtifactRef {
            path: "/work/out.png".into(),
            title: "out.png".into(),
            mime: Some("image/png".into()),
        }],
    });
    assert_eq!(r["error"]["code"], "sandbox");
    rt(&ToolContext {
        tenant_id: "d".into(),
        workspace_id: "w".into(),
        chat_id: "c".into(),
        session_id: "s".into(),
        task_id: "t".into(),
        session_token: "k".into(),
        thread_id: None,
        attachments_message_id: Some("m".into()),
    });

    let spec = rt(&SandboxSpec::new("aite-sandbox:p0"));
    assert_eq!(spec["network"], "none");
    assert_eq!(spec["workdir"], "/work");
    let parsed: ExecRequest = serde_json::from_value(json!({"code": "print(1)"})).unwrap();
    assert_eq!(
        (parsed.language, parsed.timeout_sec),
        (ExecLanguage::Python, 120)
    );
    rt(&ExecResult {
        exit_code: 124,
        stdout: "".into(),
        stderr: "[aite] 超时".into(),
        duration_ms: 5001,
        truncated: false,
        files_out: vec![FileEntry {
            path: "/work/out.png".into(),
            size: 40139,
        }],
    });

    let ev = rt(&EvidenceEvent {
        task_id: "t".into(),
        seq: 0,
        kind: EvidenceKind::TaskCreated,
        payload_hash: "0".repeat(64),
        payload_ref: None,
        payload: Some(Map::new()),
        prev_hash: GENESIS.into(),
        hash: "1".repeat(64),
        created_at: Utc::now(),
    });
    assert_eq!(ev["kind"], "task_created");
    assert_eq!(
        ev["payload_ref"],
        Value::Null,
        "内联时 payload_ref 落盘是 null"
    );
}
