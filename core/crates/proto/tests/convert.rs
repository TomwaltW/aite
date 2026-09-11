use aite_contracts::*;
use aite_proto::pb;
use aite_proto::status::{platform_error_from_status, sandbox_error_from_status};
use chrono::{TimeZone, Utc};
use serde_json::json;
use tonic::{Code, Status};

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
        sender_name: None,
        text: "画图".into(),
        raw_text: Some("@_user_1 画图".into()),
        mentioned: true,
        anchor: Anchor {
            platform: "feishu".into(),
            chat_id: "oc_1".into(),
            message_id: "om_1".into(),
            thread_id: None,
            task_no: None,
        },
        attachments: vec![Attachment {
            kind: AttachmentKind::File,
            file_key: "f".into(),
            message_id: "om_1".into(),
            name: Some("a.csv".into()),
            size: Some(3),
            mime: None,
        }],
        card_action: None,
        occurred_at: Utc.with_ymd_and_hms(2026, 9, 9, 1, 7, 0).unwrap(),
        raw:
            json!({"header": {"event_id": "e1", "n": 3, "f": 1.5, "arr": [true, null], "s": "文"}})
                .as_object()
                .unwrap()
                .clone(),
    }
}

#[test]
fn event_round_trips_through_pb() {
    let ev = event();
    let p: pb::NormalizedEvent = ev.clone().into();
    assert_eq!(p.sender_kind(), pb::SenderKind::Human);
    assert_eq!(p.kind(), pb::EventKind::Message);
    let back: NormalizedEvent = p.try_into().unwrap();
    assert_eq!(back, ev);
}

#[test]
fn pb_defects_are_rejected_not_defaulted() {
    let mut p: pb::NormalizedEvent = event().into();
    p.anchor = None;
    assert!(matches!(
        NormalizedEvent::try_from(p.clone()),
        Err(aite_proto::ConvertError::Missing { field: "anchor" })
    ));
    let mut p: pb::NormalizedEvent = event().into();
    p.sender_kind = pb::SenderKind::Unspecified as i32;
    assert!(NormalizedEvent::try_from(p).is_err());
    let mut p: pb::NormalizedEvent = event().into();
    p.occurred_at = None;
    assert!(NormalizedEvent::try_from(p).is_err());
    let mut p: pb::NormalizedEvent = event().into();
    p.tenant_id = String::new();
    assert_eq!(NormalizedEvent::try_from(p).unwrap().tenant_id, "default");
}

#[test]
fn card_and_outbound_round_trip() {
    let card = ChecklistCard {
        task_id: "t".into(),
        task_no: "#A1".into(),
        title: "x".into(),
        initiator: "张三".into(),
        started_at: "9:02".into(),
        status: CardStatus::Delivered,
        items: vec![ChecklistItemView {
            id: "c1".into(),
            text: "a".into(),
            state: ChecklistState::Done,
            note: Some("n".into()),
        }],
        footer: "f".into(),
        actions: vec![CardActionKind::Stop, CardActionKind::Evidence],
    };
    let p: pb::ChecklistCard = card.clone().into();
    assert_eq!(p.status(), pb::CardStatus::Delivered);
    assert_eq!(ChecklistCard::try_from(p).unwrap(), card);

    let t = OutboundText {
        chat_id: "c".into(),
        text: "t".into(),
        reply_to: Some("r".into()),
        in_thread: false,
    };
    assert_eq!(OutboundText::from(pb::OutboundText::from(t.clone())), t);
    let f = OutboundFile {
        chat_id: "c".into(),
        reply_to: None,
        name: "a.png".into(),
        mime: "image/png".into(),
        data: vec![1, 2, 3],
    };
    assert_eq!(OutboundFile::from(pb::OutboundFile::from(f.clone())), f);
    let h = HistoryMessage {
        message_id: "m".into(),
        sender_id: "s".into(),
        sender_kind: "bot".into(),
        sender_name: Some("B".into()),
        text: "t".into(),
        thread_id: Some("r".into()),
        created_at: Utc.with_ymd_and_hms(2026, 9, 9, 1, 7, 0).unwrap(),
    };
    assert_eq!(
        HistoryMessage::try_from(pb::HistoryMessage::from(h.clone())).unwrap(),
        h
    );
    assert_eq!(
        PlatformCapabilities::from(pb::PlatformCapabilities::from(feishu_p0())),
        feishu_p0()
    );
}

#[test]
fn sandbox_round_trip_and_zero_defaults() {
    let spec = SandboxSpec::new("img:1");
    assert_eq!(
        SandboxSpec::try_from(pb::SandboxSpec::from(spec.clone())).unwrap(),
        spec
    );
    let defaults = SandboxSpec::try_from(pb::SandboxSpec {
        image: "img:1".into(),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        (defaults.cpu, defaults.mem_mb, defaults.workdir.as_str()),
        (1.0, 1024, "/work")
    );
    assert!(
        SandboxSpec::try_from(pb::SandboxSpec {
            network: "bridge".into(),
            ..Default::default()
        })
        .is_err()
    );
    let req = ExecRequest::python("print(1)", 5);
    assert_eq!(
        ExecRequest::try_from(pb::ExecRequest::from(req.clone())).unwrap(),
        req
    );
    let res = ExecResult {
        exit_code: 0,
        stdout: "1".into(),
        stderr: String::new(),
        duration_ms: 7,
        truncated: false,
        files_out: vec![FileEntry {
            path: "/work/a".into(),
            size: 1,
        }],
    };
    assert_eq!(ExecResult::from(pb::ExecResult::from(res.clone())), res);
}

#[test]
fn status_mapping_is_frozen() {
    let e = platform_error_from_status(&Status::new(Code::Unavailable, "99991400: rate limited"));
    assert!(e.retryable);
    assert_eq!(
        (e.code.as_str(), e.message.as_str()),
        ("99991400", "rate limited")
    );
    let e = platform_error_from_status(&Status::new(Code::NotFound, "230011: msg not found"));
    assert!(!e.retryable);
    assert_eq!(e.http_status, Some(404));
    assert!(
        platform_error_from_status(&Status::new(Code::DeadlineExceeded, "timeout: 1s")).retryable
    );
    assert!(
        !platform_error_from_status(&Status::new(Code::Unimplemented, "not_implemented: x"))
            .retryable
    );
    let e = platform_error_from_status(&Status::new(Code::Internal, "no colon here"));
    assert_eq!(e.code, "grpc_internal");

    assert_eq!(
        sandbox_error_from_status(&Status::new(
            Code::Unavailable,
            "sandbox_unavailable: no docker"
        ))
        .kind,
        SandboxErrorKind::Unavailable
    );
    assert_eq!(
        sandbox_error_from_status(&Status::new(Code::NotFound, "sandbox_not_found: sb-1")).kind,
        SandboxErrorKind::NotFound
    );
    assert_eq!(
        sandbox_error_from_status(&Status::new(Code::NotFound, "file_not_found: /work/x")).kind,
        SandboxErrorKind::FileNotFound
    );
    assert_eq!(
        sandbox_error_from_status(&Status::new(
            Code::InvalidArgument,
            "sandbox_invalid_path: .."
        ))
        .kind,
        SandboxErrorKind::InvalidPath
    );
    assert_eq!(
        sandbox_error_from_status(&Status::new(Code::DeadlineExceeded, "sandbox_timeout: x")).kind,
        SandboxErrorKind::Timeout
    );
}
