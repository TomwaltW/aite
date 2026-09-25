//! 任务派发（队列 → worker）、取消的收尾（证据、卡片、沙箱）、steer 目标判定。
//! CC2 从 `plane.rs` 原样搬来；`ControlPlane::cancel_task` 的函数体搬成了
//! [`InProcessControlPlane::cancel_task_inner`]，trait 方法留在 `plane.rs` 转调。
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use serde_json::{Map, Value};

use aite_contracts::{
    CardStatus, EvidenceKind, IngressError, OutboundText, RunHooks, Session, Task, TaskStatus,
};

use crate::card::render_card;
use crate::lock;
use crate::plane::{InProcessControlPlane, Shared};
use crate::sessions::initiator_of;

/// `_run_task` 那圈 `finally`：任务是跑完了、还是压根没被领走（已取消 / 记录没了 /
/// 没配 worker），排在它名下的 steer 都不会再有人来 drain。
pub(crate) struct FinishGuard<'a> {
    pub(crate) shared: &'a Shared,
    pub(crate) task_id: &'a str,
}

impl Drop for FinishGuard<'_> {
    fn drop(&mut self) {
        // 两个容器必须在**同一个临界区**里清：只清掉 steer 就放手的话，
        // `continue_session` 的 `steer_target`（它持 owned + running）还会把这个任务
        // 挑成 steer 目标，于是一条追问排进一个再也不会被 drain 的队列。
        // 取锁顺序照本文件的全局序 owned → steer（见 `Shared` 的注释），不反向取。
        let mut owned = lock(&self.shared.owned);
        let mut steer = lock(&self.shared.steer);
        steer.remove(self.task_id);
        owned.remove(self.task_id);
    }
}

/// `run_one` 那圈 `finally`：不管是跑完、报错，还是整条 future 被 `abort()` 丢掉，
/// 队列的完成计数都要落一笔。
pub(crate) struct TaskDoneGuard<'a> {
    pub(crate) shared: &'a Shared,
}

impl Drop for TaskDoneGuard<'_> {
    fn drop(&mut self) {
        self.shared.queue.task_done();
    }
}

/// `_dispatch_task` 那圈 `finally`。
pub(crate) struct RunningGuard<'a> {
    pub(crate) shared: &'a Shared,
    pub(crate) task_id: &'a str,
}

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        lock(&self.shared.running).remove(self.task_id);
    }
}

impl InProcessControlPlane {
    /// 取消的那一句回音（`!stop` / 卡片 stop 按钮走到这里；收尾那条路 `notify=false`）。
    pub(crate) async fn notify_cancelled(
        &self,
        task: &Task,
        session: Option<&Session>,
        reply_to: Option<String>,
        chat_id: Option<String>,
    ) {
        let chat_id =
            chat_id.unwrap_or_else(|| session.map(|s| s.chat_id.clone()).unwrap_or_default());
        let msg = OutboundText {
            chat_id,
            text: format!("任务 {} 已停止。", task.task_no),
            reply_to,
            in_thread: true,
        };
        if let Err(e) = self.platform.send_text(&msg).await {
            tracing::error!(target: "aite.control", task = %task.id, error = %e, "control.notify_failed");
        }
    }

    // ---- 任务派发 --------------------------------------------------------

    pub(crate) async fn dispatch_loop(&self) {
        loop {
            let task_id = self.shared.queue.pop().await;
            self.run_one(&task_id).await;
        }
    }

    /// `with_max_parallel(n > 1)` 的派发（CC2 ②）：同一会话串行、跨会话至多 `n` 个。
    ///
    /// **结构化并发**：`run_forever(&self)` 是冻结签名、拿不到 `'static`，而 `app` 收尾靠
    /// `runner.abort()` 把在飞的全部丢掉 —— 所以在飞的任务不 `tokio::spawn`，而是这一个
    /// future 自己轮询的一组子 future。abort 时它们跟着被 drop，三个 guard 照原来的 drop
    /// 语义收尾（`task_done`、摘 `running`、清 `owned` / `steer`）。
    ///
    /// 挑任务用 `try_pop_where`：key（会话 id）正在飞的项跳过、原序留在队里，所以
    /// `pending()` / `state().queued` 仍然只数「还没开跑的」，口径不变。
    pub(crate) async fn dispatch_loop_parallel(&self, n: usize) {
        type InFlight<'a> = (String, Pin<Box<dyn Future<Output = ()> + Send + 'a>>);
        let mut in_flight: Vec<InFlight<'_>> = Vec::new();
        loop {
            // 先登记「有新任务入队」再去取：取空之后才 put 的那一下不会漏掉（同 `pop`）。
            let arrived = self.shared.queue.item_notified();
            tokio::pin!(arrived);
            arrived.as_mut().enable();
            while in_flight.len() < n {
                let busy: HashSet<String> = in_flight.iter().map(|(k, _)| k.clone()).collect();
                let Some((task_id, key)) = self.shared.queue.try_pop_where(|k| busy.contains(k))
                else {
                    break;
                };
                in_flight.push((key, Box::pin(async move { self.run_one(&task_id).await })));
            }
            // 等其一：有在飞的收尾（空出名额 / 会话不再忙），或者有新任务入队。
            std::future::poll_fn(|cx| {
                let before = in_flight.len();
                in_flight.retain_mut(|(_, fut)| fut.as_mut().poll(cx).is_pending());
                if in_flight.len() < before {
                    return Poll::Ready(());
                }
                if arrived.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(());
                }
                Poll::Pending
            })
            .await;
        }
    }

    pub(crate) async fn run_one(&self, task_id: &str) {
        // `task_done()` 必须是 drop-safe（Python 那边是 `finally`）：收尾时
        // `runner.abort()` 会在任意一个 await 点把这条 future 整个丢掉，而 `join()`
        // 等的就是这个计数 —— 漏一次，队列永远清不空，下一次优雅退出就卡满整个宽限期，
        // 而且 `pending()` 会一直报一个不存在的任务。
        let _done = TaskDoneGuard {
            shared: &self.shared,
        };
        // §3.3 任何未捕获异常都不能让进程退出
        if let Err(e) = self.run_task(task_id).await {
            tracing::error!(target: "aite.control", task = %task_id, error = %e, "control.dispatch_failed");
        }
    }

    pub(crate) async fn run_task(&self, task_id: &str) -> Result<(), IngressError> {
        let _finish = FinishGuard {
            shared: &self.shared,
            task_id,
        };
        self.dispatch_task(task_id).await
    }

    pub(crate) async fn dispatch_task(&self, task_id: &str) -> Result<(), IngressError> {
        let Some(task) = self.store.get_task(task_id).await? else {
            return Ok(());
        };
        let Some(session) = self.store.get_session(&task.session_id).await? else {
            return Ok(());
        };
        let Some(worker) = self.worker.clone() else {
            tracing::warn!(target: "aite.control", task = %task_id, "control.no_worker");
            return Ok(());
        };
        // 「查 cancelled」与「登记 running」必须在同一个临界区。Python 靠单事件循环
        // 白拿这条：`plane.py:505-511` 从判 `_cancelled` 到 `_running.add` 之间没有 await。
        // Rust 多线程 runtime 下拆成两次独立取锁就开了窗口 —— cancel_task 正好在中间
        // 抄到 running 为空，于是走「不在跑」分支写一条 cancelled 证据 + finalize，
        // 而 worker 下一步见 is_cancelled 为真又按契约自己收一次尾：同一条链两条
        // cancelled，manifest 也可能 finalize 两遍。
        // 取锁顺序与 cancel_task 侧一致（cancelled → running），全文件没有反向获取点。
        let admitted = {
            let cancelled = lock(&self.shared.cancelled);
            if cancelled.contains(task_id) {
                false
            } else {
                lock(&self.shared.running).insert(task_id.to_string());
                true
            }
        };
        if !admitted {
            return Ok(());
        }
        // 开跑前排进来的 steer 已经在 transcript 里了：`continue_session` 先 append_turn
        // 再排队，而 worker 的 `_build_messages` 是现在才去 list_turns。不清掉的话第一步
        // 开头会把同一句话再注入一遍，上下文里出现两条一样的用户发言。
        lock(&self.shared.steer).remove(task_id);
        let _running = RunningGuard {
            shared: &self.shared,
            task_id,
        };

        let initiator = initiator_of(&session);
        let hooks = RunHooks {
            drain_steer: {
                let shared = Arc::clone(&self.shared);
                let id = task_id.to_string();
                Arc::new(move || shared.drain_steer(&id))
            },
            is_cancelled: {
                let shared = Arc::clone(&self.shared);
                let id = task_id.to_string();
                Arc::new(move || lock(&shared.cancelled).contains(&id))
            },
        };
        worker.run(task, session, Some(initiator), hooks).await;
        Ok(())
    }

    // ---- 取消（!stop / 卡片 stop 按钮，§3.3）-----------------------------

    pub(crate) async fn finalize_evidence(&self, task: &mut Task, session: Option<&Session>) {
        // `finalize` 每个任务只该走一次：worker 收过尾的任务 `evidence_root_hash` 已经有值，
        // 再 finalize 一遍会把它算进这条 cancelled 之后，manifest 就和 worker 写的那份对不上了。
        if task
            .evidence_root_hash
            .as_deref()
            .is_some_and(|h| !h.is_empty())
        {
            return;
        }
        let mut extra = Map::new();
        extra.insert(
            "session_id".into(),
            Value::String(
                session
                    .map(|s| s.id.clone())
                    .unwrap_or_else(|| task.session_id.clone()),
            ),
        );
        extra.insert("task_no".into(), Value::String(task.task_no.clone()));
        extra.insert("created_by".into(), Value::String(task.created_by.clone()));
        // Task.model 建任务时取的是 config.model.model（默认空串），worker 开跑才覆盖成
        // 真模型名。没被领走就被停的任务遇上空配置就还是空的，退回本进程装着的模型名。
        extra.insert(
            "model".into(),
            Value::String(if task.model.is_empty() {
                self.model_name.clone()
            } else {
                task.model.clone()
            }),
        );
        match self.evidence.finalize(&task.id, extra).await {
            Ok(root_hash) => {
                task.evidence_root_hash = Some(root_hash);
                // 上面那次 update_task 在 finalize 之前，root_hash 得靠这一次才进库。
                task.updated_at = self.now();
                if let Err(e) = self.store.update_task(task).await {
                    tracing::error!(target: "aite.control", task = %task.id, error = %e, "control.cancel_save_failed");
                }
            }
            Err(e) => {
                tracing::error!(target: "aite.control", task = %task.id, error = %e, "control.finalize_failed");
            }
        }
    }

    /// 让 Gateway 释放它给这个 task 建的沙箱并撤掉 token。
    ///
    /// 沙箱是 Gateway 按 task_id 自己记着的（`Task.sandbox_id` 只记 worker 兜底建的那种），
    /// 契约里已经把这个方法显式化了（D2），不再靠 getattr 探测。
    pub(crate) async fn release_gateway_sandbox(&self, task_id: &str) {
        if let Some(gateway) = self.gateway.as_ref() {
            gateway.release_task(task_id).await;
        }
    }
}

impl InProcessControlPlane {
    /// `ControlPlane::cancel_task` 的函数体（CC2 ① 原样搬来，trait 方法转调它）。
    pub(crate) async fn cancel_task_inner(
        &self,
        task: Task,
        reply_to: Option<String>,
        chat_id: Option<String>,
        notify: bool,
    ) -> Task {
        let mut task = task;
        // 与 dispatch_task 侧成对：抄 running 和落 cancelled 必须是同一个临界区，
        // 否则两边会同时认为「对方没接手」，把收尾做两遍。顺序同为 cancelled → running。
        let running = {
            let mut cancelled = lock(&self.shared.cancelled);
            let r = lock(&self.shared.running).contains(&task.id);
            cancelled.insert(task.id.clone());
            r
        };

        // 传进来的 `task` 可能是一份**过期快照**，落刀之前按 id 重读一次。
        //
        // 真会撞上的是收尾那一支：`app` 的 `shutdown()` 在宽限期超时时抄一份
        // `worker.in_flight()`，抄到 `runner.abort()` 生效之间，那个任务完全可能已经
        // 自己跑完了（worker 在别的 task 上，runtime 是 multi_thread，两条线真并行）。
        // 拿旧快照往下走的后果是硬的：`status = Cancelled` + `update_task` 会把库里
        // 一条已经 `delivered` 的任务**改写成 cancelled**，卡片跟着翻成「已取消」——
        // 而用户手上的答复和文件早就收到了；`finalize_evidence` 的幂等判据读的又是
        // 快照里的 `evidence_root_hash`（旧快照那一份是空的），manifest 还会再
        // finalize 一遍，和 worker 写的那份对不上。
        //
        // 所以：库里已经是终态就一个字都不改，只把 notify 走完（卡片 stop 按钮那条路
        // 的用户点了得有回音）。多读一次库，换掉一条对用户说假话的路。
        let settled = match self.store.get_task(&task.id).await {
            Ok(Some(fresh)) if fresh.status.is_terminal() => {
                tracing::info!(
                    target: "aite.control",
                    task = %task.id,
                    status = fresh.status.as_str(),
                    "control.cancel_skipped 任务已经收场，取消不再改写它"
                );
                task = fresh;
                true
            }
            // 读不出来（store 抖动 / 已关）就按原样往下走：这条路原本就是「尽力善终」，
            // 为一次读失败放弃取消，比多写一条 cancelled 更糟。
            Err(e) => {
                tracing::warn!(target: "aite.control", task = %task.id, error = %e, "control.cancel_reread_failed");
                false
            }
            _ => false,
        };
        if settled {
            if notify {
                let session = self
                    .store
                    .get_session(&task.session_id)
                    .await
                    .ok()
                    .flatten();
                self.notify_cancelled(&task, session.as_ref(), reply_to, chat_id)
                    .await;
            }
            return task;
        }

        task.status = TaskStatus::Cancelled;
        task.updated_at = self.now();
        if let Err(e) = self.store.update_task(&task).await {
            // Python 这里会把异常抛给 handle_event；`ControlPlane::cancel_task` 的签名
            // 返回 Task（冻结契约，D5），没有错误通道 —— 不能上抛这条不是我们能改的。
            //
            // 但**落库都没成功就不该接着往下走**：继续写 cancelled 证据 + finalize，
            // 而 finalize 的 root_hash 回写同样会失败 → 下次起飞 recover_orphan_tasks
            // 把它当孤儿再 finalize 一遍（幂等守卫读的正是库里的 evidence_root_hash），
            // 两份 manifest 对不上；再回一句「已停止」更是直接对用户说假话 ——
            // 库里任务还是 created，下一条 !status 照样列着它。
            // 至少要让 `!status` 的那句进程级警告能说出「有事情没办成」。
            //
            // **计数器与 `events.dropped` 拆开了（BB1）。** 原来这里数的也是
            // `events.dropped`，两件完全不同的事压在一个名字上：那个数的是「事件在路由里
            // 炸了，可能压根没被处理」，而这一笔是「事件处理得好好的，命令也认了，
            // 只有落库这一下没成」。M4 要分的正是这种区别，混在一起就分不出来了；
            // 而且 `dropped_note()` 给的指路（去 grep `ingress.handle_failed`）对这一笔
            // 是错的 —— 它的现场是上面那行 `control.cancel_save_failed`。
            //
            // 但那句进程级警告**不能因此哑掉**：落库失败这一支是直接 `return` 的，
            // 用户那条 `!stop` 一个字的回音都收不到（连「没停成」都不说）。所以
            // `dropped_note()` 改成两个计数器一起数（见那个函数），用户照旧被告知
            // 「有事情没办成」，而 M4 拿得到分得开的两个名字。
            tracing::error!(target: "aite.control", task = %task.id, error = %e, "control.cancel_save_failed");
            self.shared.bump("control.cancel_save_failed");
            return task;
        }
        if let (Some(sandbox), Some(sandbox_id)) = (
            self.sandbox.as_ref(),
            task.sandbox_id.clone().filter(|s| !s.is_empty()),
        ) && let Err(e) = sandbox.release(&sandbox_id).await
        {
            tracing::warn!(target: "aite.control", task = %task.id, error = %e, "control.release_failed");
        }

        let session = match self.store.get_session(&task.session_id).await {
            Ok(session) => session,
            Err(e) => {
                tracing::error!(target: "aite.control", task = %task.id, error = %e, "control.cancel_session_failed");
                None
            }
        };

        // 任务正在 worker 手里跑：证据、收卡片、还沙箱都交给它下一步开头做，那边拿到的
        // 步数才是准的，也免得同一条链上写两次 cancelled。反过来，没在跑的任务 worker
        // 不会再收尾，Gateway 手上那个沙箱只能在这里还。
        if !running {
            self.release_gateway_sandbox(&task.id).await;
            let mut payload = Map::new();
            payload.insert("by".into(), Value::String("stop".into()));
            payload.insert("steps".into(), Value::from(task.steps));
            if let Err(e) = self
                .evidence
                .append(&task.id, EvidenceKind::Cancelled, payload)
                .await
            {
                tracing::error!(target: "aite.control", task = %task.id, error = %e, "control.cancel_evidence_failed");
            }
            if let (Some(session), Some(card_id)) = (
                session.as_ref(),
                task.card_id.clone().filter(|c| !c.is_empty()),
            ) {
                let card = render_card(
                    &task,
                    session,
                    &initiator_of(session),
                    CardStatus::Cancelled,
                    None,
                );
                if let Err(e) = self.platform.update_card(&card_id, &card).await {
                    tracing::warn!(target: "aite.control", task = %task.id, error = %e, "control.cancel_card_failed");
                }
            }
            self.finalize_evidence(&mut task, session.as_ref()).await;
        }
        if notify {
            self.notify_cancelled(&task, session.as_ref(), reply_to, chat_id)
                .await;
        }
        task
    }
}

/// 这句话该排给哪个任务（R6）。没有能收的就返回 `None`，调用方按「新建 task」走。
///
/// 两条判据：
///
/// 1. **只认本进程接手过的**（`owned`）。`list_active_tasks` 读的是库，杀进程重启后上一批
///    任务还挂在 `working` 上，谁也不会再领它们。
/// 2. **优先给正在跑的那个**。它的 messages 在任务开跑那一刻就定型了，不合并进去就看不到
///    这句话；还躺在队列里的任务不需要 steer —— 它开跑时 `_build_messages` 会从 transcript
///    里读到。
///
/// 多个候选时取最后创建的：§3.2 `SessionStore.list_active_tasks` 虽然承诺了顺序，但这里
/// 按 `(created_at, id)` 自己排，不吃实现的 `ORDER BY`（同毫秒时靠 uuid 字符串比大小）。
pub(crate) fn steer_target(
    active: &[Task],
    owned: &HashSet<String>,
    running: &HashSet<String>,
) -> Option<Task> {
    let owned_tasks: Vec<&Task> = active.iter().filter(|t| owned.contains(&t.id)).collect();
    if owned_tasks.is_empty() {
        return None;
    }
    let running_tasks: Vec<&Task> = owned_tasks
        .iter()
        .copied()
        .filter(|t| running.contains(&t.id))
        .collect();
    let pool = if running_tasks.is_empty() {
        &owned_tasks
    } else {
        &running_tasks
    };
    pool.iter()
        .max_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)))
        .map(|t| (*t).clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    /// 给 `steer_target` 造候选。`minutes` 决定 created_at 的先后。
    fn task(id: &str, minutes: i64) -> Task {
        let now = Utc
            .with_ymd_and_hms(2026, 9, 10, 12, 0, 0)
            .single()
            .expect("固定时间戳")
            + chrono::Duration::minutes(minutes);
        Task {
            id: id.into(),
            session_id: "s1".into(),
            task_no: format!("#A{minutes}"),
            status: TaskStatus::Created,
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
            created_by: "ou_user".into(),
            created_at: now,
            updated_at: now,
        }
    }

    fn ids(xs: &[&str]) -> HashSet<String> {
        xs.iter().map(|s| (*s).to_string()).collect()
    }

    /// 白盒测 `steer_target`（对应 Python 的 test_steer_target_prefers_the_running_task_and_ignores_orphans）。
    ///
    /// P0 的路由不会让同一会话出现两个活跃 task（`continue_session` 有活跃任务就只排
    /// steer、不新建），所以这个分支从事件面造不出来 —— 直接喂候选列表。
    #[test]
    fn steer_target_prefers_the_running_task() {
        let old = task("t-old", 0);
        let running = task("t-running", 1);
        let newest = task("t-newest", 2);
        let owned = ids(&["t-old", "t-running", "t-newest"]);
        let running_set = ids(&["t-running"]);

        // 1. 正在跑的优先；2. 结果不依赖入参顺序
        for order in [
            vec![old.clone(), running.clone(), newest.clone()],
            vec![newest.clone(), running.clone(), old.clone()],
            vec![running.clone(), newest.clone(), old.clone()],
        ] {
            assert_eq!(
                steer_target(&order, &owned, &running_set).map(|t| t.id),
                Some("t-running".to_string()),
            );
        }
    }

    #[test]
    fn steer_target_falls_back_to_the_newest_owned_task() {
        let old = task("t-old", 0);
        let running = task("t-running", 1);
        let newest = task("t-newest", 2);
        let owned = ids(&["t-old", "t-running", "t-newest"]);
        let empty = HashSet::new();

        // 没有在跑的 → 取最后创建的那个，同样不看入参顺序
        for order in [
            vec![old.clone(), newest.clone(), running.clone()],
            vec![newest.clone(), old.clone(), running.clone()],
        ] {
            assert_eq!(
                steer_target(&order, &owned, &empty).map(|t| t.id),
                Some("t-newest".to_string()),
            );
        }
    }

    #[test]
    fn steer_target_ignores_orphans() {
        let old = task("t-old", 0);
        let running = task("t-running", 1);
        let newest = task("t-newest", 2);
        let nothing = HashSet::new();

        // 全是孤儿 → None，调用方按「新建 task」走
        assert!(steer_target(&[old, running, newest], &nothing, &nothing).is_none());
        assert!(steer_target(&[], &nothing, &nothing).is_none());
    }
}
