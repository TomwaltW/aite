//! `find_session_by_thread` 的两条语义与 D8 的 `busy_timeout`（RΩ 补，审核记账 R3）。
//!
//! 这三条在合流审核里被记成「零断言」：
//!
//! * **排除 archived** —— 归档过的会话不许再被话题命中，否则用户在一个已经收档的话题里
//!   追问会续到旧摊子上；
//! * **多条取最新** —— 同一话题下有多个会话时取 `created_at` 最新的那个。排序键是
//!   `(created_at DESC, id DESC)`，而 `created_at` 那一列是**定长**的
//!   （`...Z` 永远 6 位小数），字典序才等于时间序；Python 那边 `isoformat()` 是变长的，
//!   同一秒内字典序会挑错人 —— 这也是两边唯一一处刻意不照抄的地方；
//! * **`PRAGMA busy_timeout = 5000`**（spec D8）—— 设了但没人验过，改成 0 一条都不会红。
mod common;

use aite_contracts::{SessionStatus, SessionStore};
use aite_store::{BUSY_TIMEOUT_MS, SqliteSessionStore};
use chrono::{TimeZone, Utc};
use common::{CHAT, ROOT, make_session, open_store};
use tempfile::TempDir;

fn db_path(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("data").join("aite.db")
}

#[tokio::test]
async fn find_session_by_thread_skips_archived_ones() {
    let tmp = TempDir::new().unwrap();
    let store = open_store(db_path(&tmp)).await;

    let mut old = make_session("ses_old", CHAT, Some(ROOT));
    old.status = SessionStatus::Archived;
    old.archived_at = Some(Utc.timestamp_opt(1_700_000_000, 0).unwrap());
    store.create_session(&old).await.unwrap();

    // 只有一个归档会话时：话题命中不上任何东西
    assert!(
        store
            .find_session_by_thread(CHAT, ROOT)
            .await
            .unwrap()
            .is_none(),
        "归档过的会话不该再被话题命中"
    );

    // 同一话题后来又开了一个活的：命中的必须是活的那个
    let live = make_session("ses_live", CHAT, Some(ROOT));
    store.create_session(&live).await.unwrap();
    let hit = store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .unwrap()
        .expect("该命中活的那个");
    assert_eq!(hit.id, "ses_live");
}

#[tokio::test]
async fn find_session_by_thread_takes_the_newest_of_several() {
    let tmp = TempDir::new().unwrap();
    let store = open_store(db_path(&tmp)).await;

    // 故意按「不是最新的那个最后写」的顺序落库：排序靠的是 created_at，不是插入序
    for (id, secs) in [("ses_b", 200_i64), ("ses_c", 300), ("ses_a", 100)] {
        let mut s = make_session(id, CHAT, Some(ROOT));
        s.created_at = Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap();
        s.last_active_at = s.created_at;
        store.create_session(&s).await.unwrap();
    }

    let hit = store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .unwrap()
        .expect("该命中");
    assert_eq!(hit.id, "ses_c", "多条时取 created_at 最新的那个");
}

/// 同一秒内建的多个会话：靠 `created_at` 的**微秒**分辨先后。
///
/// 这条是「为什么 Rust 侧的时间戳格式与 Python 不同」那个决定的判据：
/// 冗余列 `created_at` 用的是定长 RFC3339（永远 6 位小数 + `Z`），字典序 = 时间序。
/// Python 的 `isoformat()` 小数位数是变长的（整秒时干脆没有小数），
/// 同一秒内 `ORDER BY created_at DESC` 会挑错人。
#[tokio::test]
async fn find_session_by_thread_orders_correctly_inside_one_second() {
    let tmp = TempDir::new().unwrap();
    let store = open_store(db_path(&tmp)).await;

    // 整秒 与 同一秒 + 1 微秒：Python 的变长格式下 "…:00+00:00" > "…:00.000001+00:00"
    // 是按字典序比的，会把**更早**的那个排到前面。
    let base = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    for (id, nanos) in [("ses_sharp", 0_u32), ("ses_later", 1_000)] {
        let mut s = make_session(id, CHAT, Some(ROOT));
        s.created_at = base + chrono::Duration::nanoseconds(nanos as i64);
        s.last_active_at = s.created_at;
        store.create_session(&s).await.unwrap();
    }

    let hit = store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .unwrap()
        .expect("该命中");
    assert_eq!(hit.id, "ses_later", "同一秒内要按微秒分先后");
}

/// 话题不同就不许串（`chat_id` + `thread_id` 两个都得对上）。
#[tokio::test]
async fn find_session_by_thread_does_not_cross_chats_or_threads() {
    let tmp = TempDir::new().unwrap();
    let store = open_store(db_path(&tmp)).await;
    store
        .create_session(&make_session("ses_1", CHAT, Some(ROOT)))
        .await
        .unwrap();

    assert!(
        store
            .find_session_by_thread("oc_other", ROOT)
            .await
            .unwrap()
            .is_none(),
        "换了群不该命中"
    );
    assert!(
        store
            .find_session_by_thread(CHAT, "om_other")
            .await
            .unwrap()
            .is_none(),
        "换了话题不该命中"
    );
}

/// spec D8：`PRAGMA busy_timeout = 5000`。
///
/// Rust 里跨实例并发是**真线程**（不是 asyncio 交错），撞锁时没有这 5s 的等待窗口，
/// 第二个实例会当场 `SQLITE_BUSY` 而不是排队 —— T18 那三条并发约定就成了运气。
#[tokio::test]
async fn busy_timeout_is_five_seconds() {
    let tmp = TempDir::new().unwrap();
    let store = open_store(db_path(&tmp)).await;
    assert_eq!(BUSY_TIMEOUT_MS, 5000);
    assert_eq!(
        store.pragma_int("busy_timeout").await.unwrap(),
        BUSY_TIMEOUT_MS as i64,
        "连接上没真的设上 busy_timeout"
    );
}

/// 每一个新开的实例都要带上它（`open()` 里设，不是 `init()`）。
#[tokio::test]
async fn every_new_store_instance_sets_busy_timeout() {
    let tmp = TempDir::new().unwrap();
    let db = db_path(&tmp);
    let first = open_store(&db).await;
    first.close().await.unwrap();

    // 第二个实例：连 init() 都没调，pragma 也该已经生效
    let second = SqliteSessionStore::open(&db).expect("open");
    assert_eq!(
        second.pragma_int("busy_timeout").await.unwrap(),
        BUSY_TIMEOUT_MS as i64
    );
    second.close().await.unwrap();
}
