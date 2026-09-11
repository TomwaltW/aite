//! 派发队列 —— 对应 Python 的 `asyncio.Queue[str]` + `task_done()` / `join()`。
//!
//! 为什么不用 `tokio::sync::mpsc`：`ControlPlane` 的 `pending()` 要报队列长度、`join()`
//! 要等「队列空**且**没有在跑的」，而 mpsc 的 `Receiver` 要 `&mut self` 才能收，
//! 和 `run_forever(&self)` 对不上。`VecDeque` + `Notify` 两件事都直白。
//!
//! `unfinished` 就是 `asyncio.Queue` 的那个计数：`put` 加一、`task_done` 减一，归零时
//! 唤醒 `join`。所以 `join()` 等的是「排队的都跑完了」，不是「队列被取空了」——
//! 取出来正在跑的那个也算数（T24 的串行派发依赖这条）。
use std::collections::VecDeque;
use std::sync::Mutex;

use tokio::sync::Notify;

use crate::lock;

#[derive(Default)]
struct Inner {
    items: VecDeque<String>,
    unfinished: usize,
}

pub(crate) struct DispatchQueue {
    inner: Mutex<Inner>,
    /// 有新任务入队
    item: Notify,
    /// `unfinished` 归零
    idle: Notify,
}

impl DispatchQueue {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            item: Notify::new(),
            idle: Notify::new(),
        }
    }

    pub(crate) fn put(&self, task_id: String) {
        {
            let mut g = lock(&self.inner);
            g.items.push_back(task_id);
            g.unfinished += 1;
        }
        // notify_one 会在没人等的时候存一个许可，所以「先 put 后 await」不会丢唤醒。
        self.item.notify_one();
    }

    pub(crate) fn try_pop(&self) -> Option<String> {
        lock(&self.inner).items.pop_front()
    }

    /// 取一个任务；队列空则挂起。
    pub(crate) async fn pop(&self) -> String {
        loop {
            let notified = self.item.notified();
            tokio::pin!(notified);
            // 先登记再看队列：否则「看完发现空」与「别人 put 完 notify」之间会漏掉唤醒。
            notified.as_mut().enable();
            if let Some(id) = self.try_pop() {
                return id;
            }
            notified.await;
        }
    }

    pub(crate) fn task_done(&self) {
        let idle = {
            let mut g = lock(&self.inner);
            g.unfinished = g.unfinished.saturating_sub(1);
            g.unfinished == 0
        };
        if idle {
            self.idle.notify_waiters();
        }
    }

    pub(crate) fn len(&self) -> usize {
        lock(&self.inner).items.len()
    }

    pub(crate) fn queued_ids(&self) -> Vec<String> {
        lock(&self.inner).items.iter().cloned().collect()
    }

    /// 等到入队的任务全部 `task_done`。
    pub(crate) async fn join(&self) {
        loop {
            let notified = self.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if lock(&self.inner).unfinished == 0 {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn put_then_pop_is_fifo() {
        let q = DispatchQueue::new();
        q.put("a".into());
        q.put("b".into());
        assert_eq!(q.len(), 2);
        assert_eq!(q.queued_ids(), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(q.pop().await, "a");
        assert_eq!(q.pop().await, "b");
        assert_eq!(q.len(), 0);
    }

    #[tokio::test]
    async fn join_waits_for_task_done_not_for_pop() {
        let q = Arc::new(DispatchQueue::new());
        q.put("a".into());
        let popped = q.pop().await;
        assert_eq!(popped, "a");

        let waiter = {
            let q = Arc::clone(&q);
            tokio::spawn(async move { q.join().await })
        };
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished(), "取出来还没跑完，join 不该返回");
        q.task_done();
        tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("join 该在 task_done 之后立刻返回")
            .expect("join 任务不该 panic");
    }

    #[tokio::test]
    async fn join_returns_immediately_on_an_empty_queue() {
        let q = DispatchQueue::new();
        tokio::time::timeout(std::time::Duration::from_secs(2), q.join())
            .await
            .expect("空队列上的 join 该立刻返回");
    }

    #[tokio::test]
    async fn pop_wakes_up_on_a_later_put() {
        let q = Arc::new(DispatchQueue::new());
        let taker = {
            let q = Arc::clone(&q);
            tokio::spawn(async move { q.pop().await })
        };
        tokio::task::yield_now().await;
        q.put("late".into());
        let got = tokio::time::timeout(std::time::Duration::from_secs(2), taker)
            .await
            .expect("put 之后 pop 该醒")
            .expect("pop 任务不该 panic");
        assert_eq!(got, "late");
    }
}
