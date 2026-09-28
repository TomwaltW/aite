//! CC7 ②③：替身上的两个新旋钮 —— 能力位覆盖、messages 原文记录。
use aite_contracts::{Message, ModelPort, PlatformPort, Role, all_model_tools};
use aite_testing::{FakeModel, FakePlatform, ScriptStep, fake_p0};

#[test]
fn fake_platform_with_capabilities_overrides_default() {
    assert_eq!(FakePlatform::new().capabilities(), fake_p0());

    let mut caps = fake_p0();
    caps.platform = "dingtalk".to_string();
    caps.supports_thread = false;
    let platform = FakePlatform::new().with_capabilities(caps.clone());
    assert_eq!(platform.capabilities(), caps);
    // 覆盖只读、不记账：capabilities() 不是出入站调用
    assert_eq!(platform.calls.len(), 0);
}

fn two_finals() -> Vec<ScriptStep> {
    vec![ScriptStep::final_reply("一"), ScriptStep::final_reply("二")]
}

async fn drive(model: &FakeModel) {
    let first = vec![
        Message::text(Role::System, "你是 Aite"),
        Message::text(Role::User, "第一问 rm -rf /work"),
    ];
    model
        .chat(&first, all_model_tools(), 16, 0.0)
        .await
        .expect("第一次");
    let mut second = first.clone();
    second.push(Message::text(Role::User, "第二问"));
    model
        .chat(&second, all_model_tools(), 16, 0.0)
        .await
        .expect("第二次");
}

#[tokio::test]
async fn fake_model_records_messages_only_when_asked() {
    // 默认关：什么都不留
    let off = FakeModel::new(two_finals());
    drive(&off).await;
    assert!(off.seen_messages().is_empty(), "默认该是关的");

    // 打开：每次 chat 一份全文
    let on = FakeModel::new(two_finals()).recording_messages();
    drive(&on).await;
    let seen = on.seen_messages();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].len(), 2);
    assert_eq!(seen[1].len(), 3);
    assert_eq!(seen[0][1].content, "第一问 rm -rf /work");
    assert_eq!(seen[1][2].content, "第二问");

    // 开着时 CallLog 与关着时逐条相同（记录不进 CallLog）
    let (a, b) = (off.calls.calls(), on.calls.calls());
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.method, y.method);
        assert_eq!(x.kwargs, y.kwargs);
        assert_eq!(x.result, y.result);
        assert_eq!(x.error, y.error);
    }
}
