//! T18：§3.2 那三条带并发含义的 SessionStore 约定，压在真并发下
//! （移植 `tests/control/test_store_concurrency.py` 13 条）。
//!
//! ```text
//! append_turn   「seq 由调用方分配，重复 (session_id, seq) 报错」
//! next_task_no  「原子递增 + encode_task_no」
//! seen_event    「首次调用记录并返回 False，之后 True」
//! ```
//!
//! P0 是单 worker，但**事件投递不是单路的**：`on_event` 必须 1s 内返回（§3.3 第一条），
//! 平台重连后会一次重推一批，`handle_event` 之间没有串行化保证。
//!
//! 除了同一实例内的并发，这里还压**两个 store 实例指向同一个 .db 文件** ——
//! 每实例一把锁，跨实例完全不互斥，只剩 SQLite 自己的文件锁兜底（加 D8 的
//! `busy_timeout=5000`）。这正是 `systemctl restart` 时新旧进程叠在一起的形状。
//!
//! Python 那边是 asyncio 交错，这里是 `tokio::spawn` + `spawn_blocking` 的真线程 ——
//! 比原版更严。

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;

use aite_contracts::{SessionStore, StoreError, TaskStatus, encode_task_no};
use aite_store::{ORPHAN_RESULT_SUMMARY, SqliteSessionStore};
use common::{CHAT, numbered_turn, open_store, seed_task};
use tempfile::TempDir;

fn db_path(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("aite.db")
}

/// 并发跑一批 future，按提交顺序收结果。
async fn gather<T, F>(futs: Vec<F>) -> Vec<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handles: Vec<_> = futs.into_iter().map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        out.push(h.await.expect("任务 panic 了"));
    }
    out
}

/// 装进同一个 Vec 的异构 future（两个 async block 哪怕一模一样也不是同一个类型）。
type BoxFut<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

fn is_duplicate(r: &Result<(), StoreError>) -> bool {
    matches!(r, Err(StoreError::DuplicateTurn { .. }))
}

/// 指向同一个 .db 文件的第二个 store 实例（锁与第一个互不相识）。
async fn second_store(dir: &TempDir) -> Arc<SqliteSessionStore> {
    Arc::new(open_store(db_path(dir)).await)
}

// ---- 1. append_turn 同 seq ------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_same_seq_lets_exactly_one_win() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(open_store(db_path(&tmp)).await);

    let results = gather(
        (0..16)
            .map(|_| {
                let s = store.clone();
                async move { s.append_turn(&numbered_turn("s1", 0)).await }
            })
            .collect(),
    )
    .await;

    let winners = results.iter().filter(|r| r.is_ok()).count();
    let losers = results.iter().filter(|r| is_duplicate(r)).count();
    assert_eq!(winners, 1, "应当恰好一个成功，实际 {winners}");
    assert_eq!(losers, 15, "其余都该报 DuplicateTurn，实际 {results:?}");

    // 库里只留下一条，没写重
    let turns = store.list_turns("s1", 200).await.unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].seq, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_distinct_seq_loses_nothing() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(open_store(db_path(&tmp)).await);
    let n = 64u64;

    let results = gather(
        (0..n)
            .map(|i| {
                let s = store.clone();
                async move { s.append_turn(&numbered_turn("s2", i)).await }
            })
            .collect(),
    )
    .await;
    assert!(
        results.iter().all(|r| r.is_ok()),
        "一条都不许丢：{results:?}"
    );

    let turns = store.list_turns("s2", n as u32).await.unwrap();
    assert_eq!(
        turns.iter().map(|t| t.seq).collect::<Vec<_>>(),
        (0..n).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cross_instance_same_seq_still_raises() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(open_store(db_path(&tmp)).await);
    let other = second_store(&tmp).await;

    let mut futs: Vec<BoxFut<Result<(), StoreError>>> = Vec::new();
    for s in [store.clone(), other.clone()] {
        futs.push(Box::pin(async move {
            s.append_turn(&numbered_turn("s3", 0)).await
        }));
    }
    let results = gather(futs).await;

    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|r| is_duplicate(r)).count(), 1);
    assert_eq!(store.list_turns("s3", 200).await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_turn_error_does_not_eat_neighbours() {
    // 撞主键那一路回滚的是整条连接的当前事务。真把边上的写卷进去的话，
    // 丢的是别的会话的对话轮 —— 库里静悄悄少一行，没有任何报错。
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(open_store(db_path(&tmp)).await);
    store.append_turn(&numbered_turn("s4", 0)).await.unwrap();

    let mut futs: Vec<BoxFut<Result<(), StoreError>>> = Vec::new();
    for _ in 0..8 {
        let s = store.clone();
        futs.push(Box::pin(async move {
            s.append_turn(&numbered_turn("s4", 0)).await
        }));
    }
    for i in 0..8 {
        let s = store.clone();
        futs.push(Box::pin(async move {
            s.append_turn(&numbered_turn("s5", i)).await
        }));
    }
    let results = gather(futs).await;

    assert_eq!(results.iter().filter(|r| is_duplicate(r)).count(), 8);
    // 邻居一条不少
    let neighbours = store.list_turns("s5", 200).await.unwrap();
    assert_eq!(
        neighbours.iter().map(|t| t.seq).collect::<Vec<_>>(),
        (0..8).collect::<Vec<_>>()
    );
}

// ---- 2. next_task_no 原子性 ----------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_next_task_no_never_collides() {
    // 撞号不是小事：`!stop #A3` 是人在群里按号停任务的，两个任务同号就会停错那个。
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(open_store(db_path(&tmp)).await);
    let n = 200u64;

    let nos = gather(
        (0..n)
            .map(|_| {
                let s = store.clone();
                async move { s.next_task_no("default").await.unwrap() }
            })
            .collect(),
    )
    .await;

    let uniq: BTreeSet<_> = nos.iter().cloned().collect();
    assert_eq!(
        uniq.len(),
        n as usize,
        "撞号了：{} 个重复",
        n as usize - uniq.len()
    );
    let want: BTreeSet<_> = (1..=n).map(|i| encode_task_no(i).unwrap()).collect();
    assert_eq!(uniq, want, "恰好 1..N，不跳号");
}

#[tokio::test(flavor = "multi_thread")]
async fn cross_instance_next_task_no_never_collides() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(open_store(db_path(&tmp)).await);
    let other = second_store(&tmp).await;
    let n = 50u64;

    let mut futs: Vec<BoxFut<String>> = Vec::new();
    for _ in 0..n {
        let s = store.clone();
        futs.push(Box::pin(
            async move { s.next_task_no("default").await.unwrap() },
        ));
    }
    for _ in 0..n {
        let s = other.clone();
        futs.push(Box::pin(
            async move { s.next_task_no("default").await.unwrap() },
        ));
    }
    let nos: BTreeSet<_> = gather(futs).await.into_iter().collect();

    let want: BTreeSet<_> = (1..=2 * n).map(|i| encode_task_no(i).unwrap()).collect();
    assert_eq!(nos, want);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_task_no_stays_isolated_per_tenant() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(open_store(db_path(&tmp)).await);

    let mut futs: Vec<BoxFut<String>> = Vec::new();
    for tenant in ["t-a", "t-b"] {
        for _ in 0..32 {
            let s = store.clone();
            futs.push(Box::pin(
                async move { s.next_task_no(tenant).await.unwrap() },
            ));
        }
    }
    let mut nos = gather(futs).await;
    nos.sort();

    let mut want: Vec<String> = (1..=32u64)
        .flat_map(|i| {
            let v = encode_task_no(i).unwrap();
            [v.clone(), v]
        })
        .collect();
    want.sort();
    assert_eq!(nos, want, "两个租户各自跑完 1..32");
}

// ---- 3. seen_event 并发去重 ----------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_seen_event_returns_false_exactly_once() {
    // 两路都拿到 false 就会建两个 task：同一个问题在群里被干两遍、回两次帖。
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(open_store(db_path(&tmp)).await);
    let n = 64;

    let answers = gather(
        (0..n)
            .map(|_| {
                let s = store.clone();
                async move { s.seen_event("ev-storm").await.unwrap() }
            })
            .collect(),
    )
    .await;

    let firsts = answers.iter().filter(|a| !**a).count();
    assert_eq!(firsts, 1, "有 {firsts} 路都以为自己是第一个");
    assert_eq!(answers.iter().filter(|a| **a).count(), n - 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn cross_instance_seen_event_returns_false_exactly_once() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(open_store(db_path(&tmp)).await);
    let other = second_store(&tmp).await;

    let mut futs: Vec<BoxFut<bool>> = Vec::new();
    for s in [store, other] {
        for _ in 0..8 {
            let s = s.clone();
            futs.push(Box::pin(async move { s.seen_event("ev-x").await.unwrap() }));
        }
    }
    let answers = gather(futs).await;

    assert_eq!(answers.iter().filter(|a| !**a).count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_distinct_events_all_pass_once() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(open_store(db_path(&tmp)).await);
    let ids: Vec<String> = (0..64).map(|i| format!("ev-{i}")).collect();

    let first = gather(
        ids.iter()
            .cloned()
            .map(|id| {
                let s = store.clone();
                async move { s.seen_event(&id).await.unwrap() }
            })
            .collect(),
    )
    .await;
    assert_eq!(first, vec![false; 64]);

    let again = gather(
        ids.iter()
            .cloned()
            .map(|id| {
                let s = store.clone();
                async move { s.seen_event(&id).await.unwrap() }
            })
            .collect(),
    )
    .await;
    assert_eq!(again, vec![true; 64]);
}

// ---- recover_orphan_tasks（崩溃收尾用）------------------------------------

#[tokio::test]
async fn recover_orphan_tasks_clears_the_active_three() {
    let tmp = TempDir::new().unwrap();
    let store = open_store(db_path(&tmp)).await;
    seed_task(&store, "ses_1", "tsk_1", CHAT, "活儿").await;
    let mut task = store.list_active_tasks(CHAT).await.unwrap().remove(0);

    for live in [
        TaskStatus::Created,
        TaskStatus::Planning,
        TaskStatus::Working,
    ] {
        task.status = live;
        store.update_task(&task).await.unwrap();

        let orphans = store.recover_orphan_tasks().await.unwrap();
        assert_eq!(
            orphans.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
            [task.id.clone()]
        );
        assert_eq!(orphans[0].status, TaskStatus::Failed);
        assert_eq!(orphans[0].result_summary, ORPHAN_RESULT_SUMMARY);
        // !status 上不再挂着
        assert!(store.list_active_tasks(CHAT).await.unwrap().is_empty());

        // 落盘了，不只是内存里
        let reread = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(reread.status, TaskStatus::Failed);
        assert_eq!(reread.result_summary, ORPHAN_RESULT_SUMMARY);
    }
}

#[tokio::test]
async fn recover_orphan_tasks_leaves_finished_tasks_alone() {
    // 已经收尾的任务不许被动：delivered 的答案摘要不能被覆写成「已终止」。
    let tmp = TempDir::new().unwrap();
    let store = open_store(db_path(&tmp)).await;
    seed_task(&store, "ses_1", "tsk_1", CHAT, "活儿").await;
    let mut task = store.list_active_tasks(CHAT).await.unwrap().remove(0);

    for done in [
        TaskStatus::Delivered,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
        TaskStatus::Answering,
    ] {
        task.status = done;
        task.result_summary = "原来的摘要".to_string();
        store.update_task(&task).await.unwrap();

        assert!(store.recover_orphan_tasks().await.unwrap().is_empty());
        let reread = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(reread.status, done);
        assert_eq!(reread.result_summary, "原来的摘要");
    }
}

#[tokio::test]
async fn recover_orphan_tasks_is_idempotent_and_empty_safe() {
    // 空库上调、连着调两次，都不许炸 —— 起飞路径上的东西不能挑时候。
    let tmp = TempDir::new().unwrap();
    let store = open_store(db_path(&tmp)).await;
    assert!(store.recover_orphan_tasks().await.unwrap().is_empty());
    assert!(store.recover_orphan_tasks().await.unwrap().is_empty());
}
