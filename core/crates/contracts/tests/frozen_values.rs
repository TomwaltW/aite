use aite_contracts::*;

#[test]
fn constants() {
    assert_eq!(CONTRACT_VERSION, "p0.2");
    assert_eq!(MAX_TOOL_CONTENT_CHARS, 12_000);
    assert_eq!(DEFAULT_TOOL_TIMEOUT_SEC, 60);
    assert_eq!(MAX_EXEC_OUTPUT_CHARS, 20_000);
    assert_eq!(sandbox::EXEC_TIMEOUT_EXIT_CODE, 124);
    assert_eq!(
        ACTIVE_TASK_STATUSES,
        [
            TaskStatus::Created,
            TaskStatus::Planning,
            TaskStatus::Working
        ]
    );
}

#[test]
fn feishu_p0_capabilities() {
    let c = feishu_p0();
    assert_eq!(c.platform, "feishu");
    assert!(
        c.supports_thread && c.supports_history && c.supports_card_edit && c.inbound_file_in_group
    );
    assert!(!c.supports_passive_listen && !c.proactive_requires_prior_message);
    assert_eq!(c.card_edit_window_sec, 1_209_600);
    assert_eq!(c.outbound_rate_per_min, 60);
    // 每次调用都是新值：改一份不影响另一份
    let mut a = feishu_p0();
    a.supports_passive_listen = true;
    assert!(!feishu_p0().supports_passive_listen);
}

#[test]
fn enum_string_forms() {
    assert_eq!(SenderKind::Human.as_str(), "human");
    assert_eq!("bot".parse::<SenderKind>().unwrap(), SenderKind::Bot);
    assert!("robot".parse::<SenderKind>().is_err());
    assert_eq!(EventKind::MessageEdited.to_string(), "message_edited");
    assert_eq!(EvidenceKind::ToolResult.as_str(), "tool_result");
    assert_eq!(ToolErrorCode::InvalidArgs.as_str(), "invalid_args");
    assert_eq!(TaskStatus::AwaitingApproval.as_str(), "awaiting_approval");
    assert_eq!(EvidenceKind::ALL.len(), 10);
    assert_eq!(TaskStatus::ALL.len(), 8);
}
