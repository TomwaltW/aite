//! BB1 ②：被路由丢掉的事件要在日志里留下**名字**（`acceptance-M.md` §8 第 3 条）。
//!
//! 计数器早就有了（`events.nonhuman` / `events.duplicate` / `events.ignored`），
//! 但它们只回答「丢了几条」。M4 手上拿着的是开放平台给的一个 event_id，要问的是
//! 「这条到底有没有投递到 core、是被谁丢的」—— 那得靠日志。
//!
//! **单独一个测试文件 = 单独一个测试二进制**：`set_global_default` 一个进程只装得了一次，
//! 而这一组要收的是 INFO 级别（三条丢弃都不是故障，见 `plane.rs` 的 `drop_log` 那段），
//! 装在别的文件里会把那边的日志也一起收进来。订阅者是手写的最小实现，只用 `tracing`
//! 本身 —— 为几条测试往 `Cargo.lock` 里加 `tracing-subscriber` 不值当
//! （`core/Cargo.lock` 归 R0，各轨不该动它；范本是 `evidence/tests/torn_tail_log.rs`）。
mod support;

use std::sync::{Arc, Mutex, OnceLock};

use aite_contracts::{ControlPlane, SenderKind, SessionStore};
use support::{CHAT, Harness, active_tasks, ev};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

// --------------------------------------------------------------------------
// 手写订阅者：收 INFO 及以上，连字段一起攒
// --------------------------------------------------------------------------

/// `torn_tail_log.rs` 那个只收 message，这里**必须连字段一起收** ——
/// 本组断言的正是「日志里有没有那条事件的名字」，而名字在字段上。
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

impl Capture {
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 消息名 + 某个关键字都命中的那些行。
    fn matching(&self, name: &str, needle: &str) -> Vec<String> {
        self.lines()
            .into_iter()
            .filter(|l| l.contains(name) && l.contains(needle))
            .collect()
    }
}

#[derive(Default)]
struct LineVisitor {
    message: String,
    fields: Vec<String>,
}

impl LineVisitor {
    fn push(&mut self, field: &Field, rendered: String) {
        if field.name() == "message" {
            self.message = rendered;
        } else {
            self.fields.push(format!("{}={rendered}", field.name()));
        }
    }
}

impl Visit for LineVisitor {
    /// `%x`（Display）与字面量 message 都走这条：`DisplayValue` 的 `Debug` 直接印
    /// Display 的结果，不带引号，所以 `{value:?}` 拿到的就是原样。
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.push(field, format!("{value:?}"));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.push(field, value.to_string());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push(field, value.to_string());
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push(field, value.to_string());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push(field, value.to_string());
    }
}

impl Subscriber for Capture {
    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        *meta.level() <= Level::INFO
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut v = LineVisitor::default();
        event.record(&mut v);
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!(
                "{} {} {} {}",
                event.metadata().level(),
                event.metadata().target(),
                v.message,
                v.fields.join(" ")
            ));
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

/// 一个二进制只装一次；各条用例靠自己那个独一无二的 event_id 从里面挑自己的行。
fn capture() -> &'static Capture {
    static CAPTURE: OnceLock<Capture> = OnceLock::new();
    CAPTURE.get_or_init(|| {
        let c = Capture::default();
        tracing::subscriber::set_global_default(c.clone()).expect("装订阅者");
        c
    })
}

// --------------------------------------------------------------------------
// R1 / R2 / R8：一条规则一个名字
// --------------------------------------------------------------------------

/// 三条丢弃规则各留各的名字，**不许合并成一个大杂烩**。
///
/// 双向：
/// * 正向 —— 每条规则都得有一行 INFO，且带上被丢那条事件的 `event_id`；
/// * 反向 —— 三个消息名**互不相同**。合成一个 `control.event_dropped` 也能让正向全绿，
///   但那正是派单点名不许的形状：M4 要分的就是「平台重推的」和「投递条件没满足的」，
///   前者正常、后者多半是用户以为自己在跟 Aite 说话而 Aite 没听见。
#[tokio::test]
async fn each_dropping_rule_names_the_event_and_itself() {
    let cap = capture();
    let h = Harness::new();
    let plane = h.plane();

    // R1：机器人发的
    plane
        .handle_event(
            ev().id("ev-r1-bot")
                .sender_kind(SenderKind::Bot)
                .text("@Aite 帮我算一下")
                .message_id("om_r1")
                .build(),
        )
        .await
        .expect("非人类事件不该报错");

    // R2：同一条 event_id 推两遍
    let dup = ev().id("ev-r2-dup").message_id("om_r2").build();
    plane.handle_event(dup.clone()).await.expect("第一条");
    plane.handle_event(dup).await.expect("重推那条");

    // R8：群里的一句闲聊，既没 @ 也不在话题里
    plane
        .handle_event(
            ev().id("ev-r8-chat")
                .text("大家早")
                .mentioned(false)
                .message_id("om_r8")
                .build(),
        )
        .await
        .expect("闲聊");

    for (name, event_id) in [
        ("control.drop_nonhuman", "ev-r1-bot"),
        ("control.drop_duplicate", "ev-r2-dup"),
        ("control.drop_ignored", "ev-r8-chat"),
    ] {
        let hit = cap.matching(name, event_id);
        assert_eq!(
            hit.len(),
            1,
            "`{name}` 该有且只有一行，且带上 `{event_id}`。全部日志：{:?}",
            cap.lines()
        );
        assert!(
            hit[0].starts_with("INFO aite.control "),
            "级别是 INFO、target 是 aite.control（§7 那张表的口径）：{}",
            hit[0]
        );
    }

    // 反向：三条规则各叫各的名字，谁都不许顶替谁
    for (name, other_event) in [
        ("control.drop_nonhuman", "ev-r8-chat"),
        ("control.drop_ignored", "ev-r1-bot"),
        ("control.drop_duplicate", "ev-r8-chat"),
    ] {
        assert!(
            cap.matching(name, other_event).is_empty(),
            "`{name}` 出现在了别的规则丢的那条事件（{other_event}）上 —— \
             三条规则合并成一个名字的话，M4 就分不出「平台重推」和「没听见」了。\
             全部日志：{:?}",
            cap.lines()
        );
    }
}

// --------------------------------------------------------------------------
// R8 与那条乱序重推的追问（README §已知边界第二条）
// --------------------------------------------------------------------------

/// **BB1 ④ 的复现，外加它现在留下的痕迹。**
///
/// 形状：一条话题追问（有 `thread_id`、没 @、群消息）被平台重推在它的 root **前面**。
/// 到达那一刻话题会话还不存在，它自己又没 @ → R5/R6/R7 三个入口条件全不命中 → R8。
/// 用户那边**零回复**，而且这句话的内容彻底消失（root 随后到达时建的任务，
/// 标题和 transcript 里都只有 root 那句）。
///
/// 这条边界本轨**没有改掉**（为什么见回执 ④：一个没有上界的缓冲区是新的病）。
/// 改掉的是「丢得一声不吭」—— 现在日志里有 `mentioned=false thread=om_root`，
/// 拿着开放平台那个 event_id 一 grep 就知道它到过 core、被 R8 丢了。
///
/// 反向那一半在下一条：root 先到时**不许**有这行日志。
#[tokio::test]
async fn an_out_of_order_thread_followup_is_dropped_but_no_longer_in_silence() {
    let cap = capture();
    let h = Harness::new();
    let plane = h.plane();

    // 追问先到，它的 root（om_root_a）还没到
    plane
        .handle_event(
            ev().id("ev-followup-a")
                .text("那个图改成柱状的")
                .mentioned(false)
                .message_id("om_followup_a")
                .thread("om_root_a")
                .build(),
        )
        .await
        .expect("路由不该报错");

    // 边界本身：掉在 R8 上，库里什么都没有，用户那边一个字都没有
    assert_eq!(plane.counter("events.ignored"), 1, "该掉在 R8 上");
    assert!(
        h.store
            .find_session_by_thread(CHAT, "om_root_a")
            .await
            .expect("查会话")
            .is_none(),
        "root 还没到，话题会话本来就不存在 —— 这正是它掉进 R8 的原因"
    );
    assert!(h.platform.texts().is_empty(), "用户那边零回复");
    assert!(h.platform.reactions().is_empty(), "连个表情都没有");

    // 留痕：这一行就是 BB1 加的那个
    let hit = cap.matching("control.drop_ignored", "ev-followup-a");
    assert_eq!(
        hit.len(),
        1,
        "被丢的追问该在日志里留下名字。全部日志：{:?}",
        cap.lines()
    );
    let line = &hit[0];
    assert!(
        line.contains("thread=om_root_a"),
        "得带上它想接进去的那个话题 —— 没有它，这行日志分不出「闲聊」和「追问丢了」：{line}"
    );
    assert!(
        line.contains("mentioned=false"),
        "得说清为什么没走 R7：{line}"
    );
}

/// 反向那一半：**顺序正常时不许有这行日志。**
///
/// 没有这一条的话，上面那条是恒真断言 —— 把 `drop_log::ignored` 挪到 `route` 的开头
/// 无条件打一行，上面全绿，而日志会变成「每条事件都报一次丢弃」的噪音，
/// 比没有还糟（§7 那张表是排障时第一个被信的东西）。
#[tokio::test]
async fn a_followup_that_arrives_after_its_root_is_not_logged_as_dropped() {
    let cap = capture();
    let h = Harness::new();
    let plane = h.plane();

    // root 先到（@ 了 Aite），再来追问 —— 这是飞书的常态
    plane
        .handle_event(
            ev().id("ev-root-b")
                .text("画个趋势图")
                .message_id("om_root_b")
                .build(),
        )
        .await
        .expect("root");
    plane
        .handle_event(
            ev().id("ev-followup-b")
                .text("那个图改成柱状的")
                .mentioned(false)
                .message_id("om_followup_b")
                .thread("om_root_b")
                .build(),
        )
        .await
        .expect("追问");

    assert_eq!(
        active_tasks(&h.store, CHAT).await.len(),
        1,
        "追问该并进 root 那个话题，不该另起一个任务"
    );
    assert!(
        cap.matching("control.drop_ignored", "ev-followup-b")
            .is_empty(),
        "顺序正常的追问被记成了「丢弃」—— 这行日志得只在真丢的时候出现。全部日志：{:?}",
        cap.lines()
    );
}
