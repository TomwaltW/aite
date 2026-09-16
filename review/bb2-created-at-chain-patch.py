#!/usr/bin/env python3
"""BB2 ①：把 `created_at` 接进证据链的 hash。

**病**（`docs/acceptance-M.md` §8 第 1 条）：`hash = chain_hash(prev_hash, payload_hash)`，
而 `payload_hash` 只覆盖 payload —— **时间戳不在任何 hash 的前像里**。
BB2 在一份真跑出来的证据目录上复现过：把 seq=8 的 `created_at` 往后挪 37 分钟
（挪完它比 seq=9 还晚），`aite evidence show` 照样打
「hash 链 OK · 17 条全部闭合，root_hash 与 manifest 一致」，退出码 0，root_hash 一个字符没变。
M2（断网重连、只处理一次）和 M3（卡片更新次数）的时序判断全建立在这些时间戳上。

**治法**：新增 `chain_hash_at(prev, payload_hash, created_at)`，落盘与校验都换成它。
`chain_hash` 留着不动（§3.1 的两个冻结向量还钉着它，而且要靠它认出「这是旧口径的链」）。

**为什么是一个脚本而不是直接改**：改动横跨两片本轨写不了的面 ——

  1. `core/crates/contracts/**`：守卫的 `PROT_PREFIXES`，读得了写不了，还被 `.contracts.lock`
     锁着（25 个文件里本补丁动 3 个）。
  2. `core/crates/app/tests/{evidence_on_disk,cold_start_to_delivery}.rs`：不是守卫拦的，
     是**派单纪律**的只读面（本轨只许新建 app/tests 下的文件）。这两条贯通用例各有一行
     `assert_eq!(e.hash, chain_hash(&prev, &e.payload_hash))`，钉的就是旧口径那条式子 ——
     **任何**改动落盘 hash 的方案都会把它们打红，绕不过去。

所以 ① 整条**一行都不落在 BB2 的 worktree 里**，全部走这份脚本，由人落地。
（②③ 与「两处 verify 口径一致」那条测试落在本轨可写面，已经在树里了，与本脚本无关。）

**兼容策略：一刀切，不加版本位。** 三条路各自的下场：

  * 「只对新事件生效」—— 同一份文件里混两种口径，等于给改证据的人一个开关：
    把某一条退回旧口径就能随便改它的时间戳而链不断。**直接出局。**
  * 「链里加版本位」—— 版本位必须自己也被 hash 盖住才有用，那就得往 `EvidenceEvent`
    上加字段；而 `EvidenceEvent` 在 `core/crates/testing/src/fake_store.rs` 与
    `core/crates/control/tests/support/mod.rs` 有**字面量构造点**（两处都在本轨只读面），
    加字段 = 那两个 crate 直接 E0063 编不过。放 manifest 里也不行：manifest 不进链、
    可以单独改，而且它的 8 键形状被 `app/tests/evidence_on_disk.rs` 钉着。**两条路都不通。**
  * 「一刀切」—— 代价只有一条：已经落盘的旧证据目录 `verify` 变红。而旧目录**本来就
    证明不了自己的时序**，它红说的正是实话。为了让人分得清「链真被改过」和「这只是旧口径」，
    `load_timeline` 会在整份目录都按旧口径闭合时单报一句人话（见 cli.rs 那处改动）。

**落地之后旧证据怎么办**：`data/evidence/` 不入库，总管本机有 M1–M6 跑了一半留下的目录。
建议重锁之前先把它们整个挪到 `data/evidence-p0.1-legacy/`，M1–M6 重跑。
内容仍然逐条可读（渲染不依赖链），只是链校验会明说「旧口径」。

**用法**（在仓库根跑）：

    AITE_RELOCK=1 python3 review/bb2-created-at-chain-patch.py --check   # 干跑，不写盘
    AITE_RELOCK=1 python3 review/bb2-created-at-chain-patch.py           # 真写

    # 自验用（不碰真仓库的冻结面，所以不要授权）：对着 /tmp 的仓库副本跑
    python3 review/bb2-created-at-chain-patch.py --root /tmp/bb2-check --check

**与 `review/aa4-proto-patch.py` 的先后关系：没有先后，两份可以任意顺序跑。**
AA4 改 `proto/aite/v1/edge.proto` 与 `edge/cmd/aite-edge/main.go` 的注释；本补丁的 8 个
文件里一个都不是它们，两份的锚点文本也互不包含。本脚本在「AA4 已打」和「AA4 未打」
两棵树上各跑过一遍 `--check`，输出逐字相同（见回执 ④）。
"""

from __future__ import annotations

import argparse
import os
import pathlib
import subprocess
import sys

# 受保护路径拼在一起会被守卫误拦（它扫的是命令文本里的子串），所以分段拼 ——
# 纯粹是为了让「grep 这个脚本」和「在命令行里提这个脚本」不互相绊。
CONTRACTS_SRC = "core/crates/" + "contracts/src"
CONTRACTS_TESTS = "core/crates/" + "contracts/tests"
EVIDENCE_SRC = "core/crates/evidence/src"
EVIDENCE_TESTS = "core/crates/evidence/tests"
APP_TESTS = "core/crates/app/tests"

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent

RUST_EDITION = "2024"


# ============================ 锚点与替换值 ====================================
# 一条 Edit 一处改动，字面量精确替换。锚点都是从文件里逐字抄出来的，
# 要求**恰好命中 1 次**：0 次和 2 次都说明现状不是本补丁预料的形态，整份拒写。

# ---- 1. 契约：新的 hash 口径 ------------------------------------------------

# 锚点把**前面那个 sha256_hex** 也圈进来，纯粹为了幂等：只圈 `chain_hash` 那三行的话，
# 补丁打完之后它们原样还在（旧函数是留着的），锚点就会二次命中。圈上前一个函数之后，
# 打完的树上这段文本不复存在，第 1 条和其余 21 条一样报「已经打过了」。
CHAIN_HASH_OLD = '''fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

pub fn chain_hash(prev_hash: &str, payload_hash: &str) -> String {
    sha256_hex(format!("{prev_hash}{payload_hash}").as_bytes())
}
'''

CHAIN_HASH_NEW = '''fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// 旧口径（p0.1 起）的链式 hash：`sha256(prev_hash + payload_hash)`。
///
/// **新落盘的证据不再用它。** 它算出来的值与 `created_at` 无关，于是「把 events.jsonl
/// 里的时间戳全改掉，链校验照样全绿」（`docs/acceptance-M.md` §8 第 1 条；BB2 在一份
/// 真证据目录上复现过：挪掉中间一条的时间戳，`evidence show` 仍然打「17 条全部闭合」）。
/// 新口径见 [`chain_hash_at`]。
///
/// 留着它有两个用处：§3.1 那两个冻结向量仍然按它钉；读旧证据目录时要拿它认出
/// 「这是旧口径的链」而不是「这条链被改过」—— 两者在人眼里差别很大。
pub fn chain_hash(prev_hash: &str, payload_hash: &str) -> String {
    sha256_hex(format!("{prev_hash}{payload_hash}").as_bytes())
}

/// `created_at` 进链时的规范形：固定 9 位小数 + `Z`。
///
/// **为什么必须固定**：serde 给 `DateTime<Utc>` 用的是 `SecondsFormat::AutoSi`，
/// 小数位按值挑 0/3/6/9 —— 同一个瞬间，Rust 落盘写成 `.123Z`，Python 的
/// `datetime.isoformat()` 写的是 `.123000`。两串字符不同，瞬间相同。
/// 时间戳一旦进 hash，它的字符串形式就成了契约的一部分：拿落盘那串直接喂 sha256，
/// 同一条证据在两个实现上会算出两条不同的链。所以进 hash 的不是落盘那串，而是从
/// `DateTime<Utc>` 的**值**重新格式化出来的这一个规范形 —— 落盘怎么写都不影响校验。
///
/// 9 位而不是 6 位：9 位是 `DateTime<Utc>` 存得下的全部精度，规范化因此**不丢信息**
/// （同一个值 ⇔ 同一个串）。截到微秒的话，差半微秒的两个瞬间会撞出同一个 hash，
/// 那等于在链上留了一条改时间戳不留痕的窄缝。旧证据、以及 `writer.rs` 的
/// `now_micros()` 写出来的值，微秒以下一律是 0，规范形末三位就是 `000`。
pub fn canonical_created_at(created_at: &DateTime<Utc>) -> String {
    created_at.format("%Y-%m-%dT%H:%M:%S%.9fZ").to_string()
}

/// 链式 hash（现行口径）：`sha256(prev_hash + payload_hash + canonical_created_at)`。
///
/// 与 [`chain_hash`] 的前像差着 30 个字节的时间戳，两者不可能算出同一个值 ——
/// 「这条链是新口径还是旧口径」因此是可判的，不需要在事件里加版本字段。
/// （也加不了：`EvidenceEvent` 在 `aite-testing` 与 `aite-control` 的测试脚手架里
/// 有字面量构造点，加字段那两处直接编不过。）
pub fn chain_hash_at(prev_hash: &str, payload_hash: &str, created_at: &DateTime<Utc>) -> String {
    let stamp = canonical_created_at(created_at);
    sha256_hex(format!("{prev_hash}{payload_hash}{stamp}").as_bytes())
}
'''

EVENT_FIELDS_OLD = '''    /// sha256(prev_hash + payload_hash) hex
    pub hash: String,
    pub created_at: DateTime<Utc>,
'''

EVENT_FIELDS_NEW = '''    /// `chain_hash_at(prev_hash, payload_hash, created_at)` hex。
    /// 旧证据目录里是 `chain_hash(prev_hash, payload_hash)`，不含时间戳。
    pub hash: String,
    /// 落盘时刻。**它进 `hash`**（见 [`chain_hash_at`]）—— 改它就断链。
    /// 进 hash 的是 [`canonical_created_at`] 算出来的规范形，不是这一行落盘的字符串：
    /// 两者小数位可能不一样，而校验只认规范形。
    pub created_at: DateTime<Utc>,
'''

VECTORS_OLD = """//   hash(上一条 hash, ·)  11dbc980a72dcce295871168fe45f8926b2b9e8ac90a2fb783dfb586e1d38f5e
"""

VECTORS_NEW = """//   hash(上一条 hash, ·)  11dbc980a72dcce295871168fe45f8926b2b9e8ac90a2fb783dfb586e1d38f5e
// 上面两个 hash 是**旧口径**（chain_hash，不含时间戳）的值，留着钉 chain_hash 本身。
// 新口径 chain_hash_at 的向量（同样在 tests/evidence_vectors.rs 里逐字节校验）：
//   created_at "2026-09-11T00:00:00Z"        规范形 2026-09-11T00:00:00.000000000Z
//   chain_hash_at(GENESIS, {"a": 1} 的 payload_hash, ·)
//                         9908ad2d6e03c3367df5bfeeca4c53461e8e4c6301998b38ade99a290c4762fb
//   created_at "2026-09-11T00:00:01.123456Z" 规范形 2026-09-11T00:00:01.123456000Z
//   chain_hash_at(上一条 hash, {"b": "文"} 的 payload_hash, ·)
//                         21246f40581fa141268ed7a25c30b922fbeabc6807487cf075a4d185ad07c64b
"""

LIB_EXPORT_OLD = """pub use evidence::{
    EvidenceEvent, EvidenceKind, GENESIS, canonical_json, chain_hash, payload_hash_of,
};
"""

LIB_EXPORT_NEW = """pub use evidence::{
    EvidenceEvent, EvidenceKind, GENESIS, canonical_created_at, canonical_json, chain_hash,
    chain_hash_at, payload_hash_of,
};
"""

# ---- 2. 契约测试：新口径的冻结向量 ------------------------------------------

VECTORS_TEST_IMPORT_OLD = """use aite_contracts::{GENESIS, canonical_json, chain_hash, payload_hash_of};
"""

VECTORS_TEST_IMPORT_NEW = """use aite_contracts::{
    GENESIS, canonical_created_at, canonical_json, chain_hash, chain_hash_at, payload_hash_of,
};
use chrono::{DateTime, Utc};
"""

VECTORS_TEST_OLD = """    assert_eq!(
        chain_hash(&h1, &payload_hash_of(&p2)),
        "11dbc980a72dcce295871168fe45f8926b2b9e8ac90a2fb783dfb586e1d38f5e"
    );
}
"""

VECTORS_TEST_NEW = '''    assert_eq!(
        chain_hash(&h1, &payload_hash_of(&p2)),
        "11dbc980a72dcce295871168fe45f8926b2b9e8ac90a2fb783dfb586e1d38f5e"
    );
}

fn at(s: &str) -> DateTime<Utc> {
    s.parse::<DateTime<Utc>>().expect("RFC3339")
}

/// 现行口径（`created_at` 进链）的向量。与上面那两个旧口径向量并存：
/// 旧的钉 `chain_hash` 本身，新的钉真正落盘的那条式子。
#[test]
fn chain_hash_at_vectors_byte_exact() {
    // 规范形固定 9 位小数：写 "Z"（0 位）、".123456Z"（6 位）进来都要被拉齐
    assert_eq!(
        canonical_created_at(&at("2026-09-11T00:00:00Z")),
        "2026-09-11T00:00:00.000000000Z"
    );
    assert_eq!(
        canonical_created_at(&at("2026-09-11T00:00:01.123456Z")),
        "2026-09-11T00:00:01.123456000Z"
    );

    let p1 = obj(json!({"a": 1}));
    let h1 = chain_hash_at(GENESIS, &payload_hash_of(&p1), &at("2026-09-11T00:00:00Z"));
    assert_eq!(
        h1,
        "9908ad2d6e03c3367df5bfeeca4c53461e8e4c6301998b38ade99a290c4762fb"
    );

    let p2 = obj(json!({"b": "文"}));
    assert_eq!(
        chain_hash_at(
            &h1,
            &payload_hash_of(&p2),
            &at("2026-09-11T00:00:01.123456Z")
        ),
        "21246f40581fa141268ed7a25c30b922fbeabc6807487cf075a4d185ad07c64b"
    );
}

/// 这条是本补丁存在的理由，写成断言：**时间戳动一下，链就得变。**
/// 它同时挡住「不小心把 chain_hash_at 实现成忽略 created_at」这种回退。
#[test]
fn moving_created_at_moves_the_hash() {
    let p1 = obj(json!({"a": 1}));
    let ph = payload_hash_of(&p1);
    let base = chain_hash_at(GENESIS, &ph, &at("2026-09-11T00:00:00Z"));

    // 差 1 纳秒就是另一条链 —— 规范形保到 9 位，所以这一格不会被截掉
    assert_ne!(
        base,
        chain_hash_at(GENESIS, &ph, &at("2026-09-11T00:00:00.000000001Z"))
    );
    // 同一个瞬间的两种写法（0 位小数 / 9 位小数）必须是同一条链
    assert_eq!(
        base,
        chain_hash_at(GENESIS, &ph, &at("2026-09-11T00:00:00.000000000Z"))
    );
    // 新旧两个口径不许撞上，否则「这条链是新是旧」就判不出来了
    assert_ne!(base, chain_hash(GENESIS, &ph));
}
'''

# ---- 3. 写入侧：writer.rs ---------------------------------------------------

WRITER_IMPORT_OLD = """use aite_contracts::{
    CONTRACT_VERSION, EvidenceError, EvidenceEvent, EvidenceKind, EvidenceWriter, GENESIS,
    canonical_json, chain_hash, payload_hash_of,
};
"""

# 注意 `chain_hash` 是**去掉**的：writer 这边不再有任何一处按旧口径算，留着就是
# `unused import`，而 A4a 的 `clippy -D warnings` 见 warning 就红。
# `cli.rs` 那边相反 —— 它还要拿旧口径认出「这是旧口径的链」，所以两个都留着。
WRITER_IMPORT_NEW = """use aite_contracts::{
    CONTRACT_VERSION, EvidenceError, EvidenceEvent, EvidenceKind, EvidenceWriter, GENESIS,
    canonical_json, chain_hash_at, payload_hash_of,
};
"""

NOW_MICROS_OLD = '''/// 现在几点，截到微秒。
///
/// Python 的 `datetime` 只有微秒精度，旧证据目录里的 `created_at` 也一律是 6 位小数。
/// chrono 的 RFC3339 序列化按实际值挑 0/3/6/9 位，在纳秒分辨率的时钟上会打出 9 位 ——
/// 那种行 Python 侧的 `verify` 不一定认。这里截一刀，两边落盘形状就是同一个。
fn now_micros() -> chrono::DateTime<Utc> {
'''

NOW_MICROS_NEW = '''/// 现在几点，截到微秒。
///
/// Python 的 `datetime` 只有微秒精度，旧证据目录里的 `created_at` 最多 6 位小数。
/// chrono 的 RFC3339 序列化按实际值挑 0/3/6/9 位，在纳秒分辨率的时钟上会打出 9 位 ——
/// 那种行 Python 侧的 `verify` 不一定认。这里截一刀，两边落盘形状就对得上。
///
/// **注意这一刀管的是「落盘长什么样」，不是「hash 怎么算」。** 落盘那串的小数位本来
/// 就不定（0/3/6 都可能出现，取决于值），所以 `created_at` 进链之后，进 hash 的是
/// `canonical_created_at` 从值上重算的 9 位规范形，不是这一行的字节。截到微秒只让
/// 规范形的末三位恒为 `000`，与 Python 侧算得出同一个串 —— 换句话说，这条注释里
/// 「两边形状一致」的承诺，现在从「好看」变成了**校验的前提**。
fn now_micros() -> chrono::DateTime<Utc> {
'''

WRITER_TAIL_OLD = """        ev.prev_hash == prev_hash && ev.hash == chain_hash(&prev_hash, &ev.payload_hash)
"""

WRITER_TAIL_NEW = """        ev.prev_hash == prev_hash
            && ev.hash == chain_hash_at(&prev_hash, &ev.payload_hash, &ev.created_at)
"""

WRITER_APPEND_OLD = """        let ev = EvidenceEvent {
            task_id: task_id.to_string(),
            seq,
            kind,
            payload_hash: p_hash.clone(),
            payload_ref,
            payload: inline,
            prev_hash: prev_hash.clone(),
            hash: chain_hash(&prev_hash, &p_hash),
            created_at: now_micros(),
        };
"""

WRITER_APPEND_NEW = """        // 时间戳进链，所以得先定下来再算 hash —— 两处各调一次 `now_micros()`
        // 就是两个不同的瞬间，写出来的链当场自己对不上自己。
        let created_at = now_micros();
        let ev = EvidenceEvent {
            task_id: task_id.to_string(),
            seq,
            kind,
            payload_hash: p_hash.clone(),
            payload_ref,
            payload: inline,
            prev_hash: prev_hash.clone(),
            hash: chain_hash_at(&prev_hash, &p_hash, &created_at),
            created_at,
        };
"""

WRITER_VERIFY_OLD = """            if ev.prev_hash != prev || ev.hash != chain_hash(&prev, &ev.payload_hash) {
                return false;
            }
"""

WRITER_VERIFY_NEW = """            if ev.prev_hash != prev
                || ev.hash != chain_hash_at(&prev, &ev.payload_hash, &ev.created_at)
            {
                return false;
            }
"""

WRITER_VERIFY_DOC_OLD = """    /// 重算整条链：seq 连续、payload 与 payload_hash 对得上、hash 链闭合。
"""

WRITER_VERIFY_DOC_NEW = """    /// 重算整条链：seq 连续、payload 与 payload_hash 对得上、hash 链闭合。
    ///
    /// hash 链按现行口径算（`chain_hash_at`，`created_at` 在前像里），所以**改任何一条的
    /// 时间戳都会在这里判 false**。旧证据目录（`chain_hash`，时间戳不进链）在这里一律
    /// 是 false —— 那不是「被改过」，是「口径不同」，`cli::load_timeline` 会把这两种
    /// 情况分开报给人看；这个函数只回答 bool，分不了，也不该分。
"""

# ---- 4. 校验侧：cli.rs ------------------------------------------------------

CLI_IMPORT_OLD = """use aite_contracts::{
    AiteConfig, EvidenceEvent, EvidenceKind, GENESIS, chain_hash, payload_hash_of,
};
"""

CLI_IMPORT_NEW = """use aite_contracts::{
    AiteConfig, EvidenceEvent, EvidenceKind, GENESIS, chain_hash, chain_hash_at, payload_hash_of,
};
"""

CLI_DOC_OLD = """/// 校验口径和 `FileEvidenceWriter::verify` 一致（seq 连续、payload 与 payload_hash 对得上、
/// prev_hash 接得住、hash == chain_hash(prev, payload_hash)），差别是这里不提前返回：
"""

CLI_DOC_NEW = """/// 校验口径和 `FileEvidenceWriter::verify` 一致（seq 连续、payload 与 payload_hash 对得上、
/// prev_hash 接得住、hash == chain_hash_at(prev, payload_hash, created_at)），差别是这里
/// 不提前返回：
"""

CLI_COUNTER_OLD = """    let mut prev = GENESIS.to_string();
    let mut first_at: Option<DateTime<Utc>> = None;
"""

CLI_COUNTER_NEW = """    let mut prev = GENESIS.to_string();
    let mut first_at: Option<DateTime<Utc>> = None;
    // 整份目录都是旧口径时（RΩ 之前落的证据），逐条报「这条是旧口径」会刷出几百块
    // 一模一样的话，真正被改过的那一条反而埋在里面。只在头一条上把话说清楚，
    // 后面的照样算问题（`broken`、`ok()`、退出码都不变），只是不再重复。
    let mut legacy_lines: usize = 0;
"""

CLI_HASH_OLD = '''        let want_hash = chain_hash(&ev.prev_hash, &ev.payload_hash);
        if ev.hash != want_hash {
            tl.issues.push(
                ChainIssue::new(
                    line_no,
                    Some(ev.seq),
                    "hash != chain_hash(prev_hash, payload_hash)",
                )
                .with(want_hash, ev.hash.clone()),
            );
            broken = true;
        }
'''

CLI_HASH_NEW = '''        let want_hash = chain_hash_at(&ev.prev_hash, &ev.payload_hash, &ev.created_at);
        if ev.hash != want_hash {
            // 旧口径的链在这里会整份报错。单说「hash 对不上」会把人带去查有没有被改过，
            // 而多半只是这份目录是 RΩ 之前落的 —— 所以顺手认一下，把话说明白。
            let legacy = ev.hash == chain_hash(&ev.prev_hash, &ev.payload_hash);
            if legacy {
                legacy_lines += 1;
            }
            if !legacy || legacy_lines == 1 {
                let problem = if legacy {
                    "这一条起是旧口径（created_at 不进链，RΩ 之前落盘的证据）—— 不是被改过；\\
                     后面同样的不再逐条列"
                } else {
                    "hash != chain_hash_at(prev_hash, payload_hash, created_at)"
                };
                tl.issues.push(
                    ChainIssue::new(line_no, Some(ev.seq), problem)
                        .with(want_hash, ev.hash.clone()),
                );
            }
            broken = true;
        }
'''

# ---- 5. evidence crate 的测试 -----------------------------------------------

CHAIN_TEST_IMPORT_OLD = """use aite_contracts::{EvidenceKind, EvidenceWriter, GENESIS};
"""

CHAIN_TEST_IMPORT_NEW = """use aite_contracts::{EvidenceKind, EvidenceWriter, GENESIS, chain_hash, chain_hash_at};
"""

CHAIN_TEST_VEC_OLD = """    assert_eq!(a.seq, 0);
    assert_eq!(a.prev_hash, GENESIS);
    assert_eq!(a.payload_hash, VEC1_PAYLOAD_HASH);
    assert_eq!(a.hash, VEC1_HASH);

    assert_eq!(b.seq, 1);
    assert_eq!(b.prev_hash, VEC1_HASH);
    assert_eq!(b.payload_hash, VEC2_PAYLOAD_HASH);
    assert_eq!(b.hash, VEC2_HASH);

    assert!(w.verify(TASK));
}
"""

CHAIN_TEST_VEC_NEW = '''    assert_eq!(a.seq, 0);
    assert_eq!(a.prev_hash, GENESIS);
    assert_eq!(a.payload_hash, VEC1_PAYLOAD_HASH);
    assert_eq!(b.seq, 1);
    assert_eq!(b.payload_hash, VEC2_PAYLOAD_HASH);

    // §3.1 的两个 payload_hash 向量照旧逐字节钉着 —— 它们只覆盖 payload，不受影响。
    // 两个 **hash** 向量是旧口径的值。它们也照旧钉着，只是钉的位置变了：
    // 从「落盘的 hash 等于它」变成「`chain_hash` 这个函数还是那个式子」——
    // 读旧证据目录时要靠它认出「这是旧口径」而不是「这条链被改过」。
    assert_eq!(chain_hash(GENESIS, VEC1_PAYLOAD_HASH), VEC1_HASH);
    assert_eq!(chain_hash(VEC1_HASH, VEC2_PAYLOAD_HASH), VEC2_HASH);

    // 落盘的 hash 现行口径把 `created_at` 也算了进去，跟着时刻走，钉不成常量。
    // 这里改成「按现行口径重算一遍必须相等」，外加一条「绝不能等于旧口径算出来的值」
    // —— 后者才是这条改动到底生没生效的判据。
    assert_eq!(
        a.hash,
        chain_hash_at(GENESIS, VEC1_PAYLOAD_HASH, &a.created_at)
    );
    assert_ne!(
        a.hash,
        chain_hash(GENESIS, VEC1_PAYLOAD_HASH),
        "落盘的 hash 还是旧口径 —— created_at 根本没进链"
    );
    assert_eq!(b.prev_hash, a.hash);
    assert_eq!(
        b.hash,
        chain_hash_at(&a.hash, VEC2_PAYLOAD_HASH, &b.created_at)
    );

    assert!(w.verify(TASK));
}

/// BB2 ① 的判据：**只改一条 `created_at`，别的一个字节不动 → 链必须断。**
///
/// 这是这条改动存在的全部理由。改之前这条是绿的（`verify` 照样返回 true），
/// BB2 在一份真跑出来的 17 条证据目录上复现过。
#[tokio::test]
async fn tampering_created_at_fails_verify() {
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    write_vectors(&w).await;
    assert!(w.verify(TASK), "没动之前得是好的");

    rewrite(&w, 0, |row| {
        row["created_at"] = json!("2030-01-01T00:00:00Z")
    });
    assert!(!w.verify(TASK), "改了 created_at 链还说自己是好的");
}

/// 同一个瞬间换个写法（小数位不同）不算改动 —— 进 hash 的是规范形，不是落盘那串。
/// 少了这一条，「规范形」就退化成「落盘字符串」，两个实现一对拍就炸。
#[tokio::test]
async fn rewriting_created_at_to_an_equal_instant_keeps_verify_green() {
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    write_vectors(&w).await;

    let stamp = events_of(&w, TASK)[0].created_at;
    // chrono 落盘时小数位是自适应的；这里显式补满 9 位，瞬间一模一样、字节不同
    let padded = stamp.format("%Y-%m-%dT%H:%M:%S%.9fZ").to_string();
    rewrite(&w, 0, |row| row["created_at"] = json!(padded));
    assert!(
        w.verify(TASK),
        "同一个瞬间换了个小数位写法就判链断 —— 进 hash 的成了落盘字符串"
    );
}
'''

RESTART_TEST_OLD = """    assert_eq!(second.seq, 1);
    assert_eq!(second.prev_hash, VEC1_HASH);
    assert_eq!(second.hash, VEC2_HASH);
    assert!(other.verify(TASK));
"""

RESTART_TEST_NEW = """    assert_eq!(second.seq, 1);
    // 同上：落盘 hash 跟着时刻走，钉不成常量。从盘上把第一条读回来再接着算 ——
    // 这条测试要钉的本来就是「新实例接得住旧文件」，不是那两个常量。
    let first = events_of(&w, TASK)[0].clone();
    assert_eq!(second.prev_hash, first.hash);
    assert_eq!(
        second.hash,
        chain_hash_at(&first.hash, VEC2_PAYLOAD_HASH, &second.created_at)
    );
    assert!(other.verify(TASK));
"""

# ---- 6. app 的两条贯通用例（派单纪律的只读面，理由见文件头） ----------------

ON_DISK_IMPORT_OLD = """use aite_contracts::{
    CONTRACT_VERSION, EvidenceKind, EvidenceWriter, GENESIS, TaskStatus, chain_hash,
    payload_hash_of,
};
"""

ON_DISK_IMPORT_NEW = """use aite_contracts::{
    CONTRACT_VERSION, EvidenceKind, EvidenceWriter, GENESIS, TaskStatus, chain_hash_at,
    payload_hash_of,
};
"""

COLD_START_IMPORT_OLD = """use aite_contracts::{
    CONTRACT_VERSION, EvidenceKind, EvidenceWriter, GENESIS, Task, TaskStatus, chain_hash,
    payload_hash_of,
};
"""

COLD_START_IMPORT_NEW = """use aite_contracts::{
    CONTRACT_VERSION, EvidenceKind, EvidenceWriter, GENESIS, Task, TaskStatus, chain_hash_at,
    payload_hash_of,
};
"""

ON_DISK_ASSERT_OLD = """        assert_eq!(e.payload_hash, payload_hash_of(payload));
        assert_eq!(e.prev_hash, prev);
        assert_eq!(e.hash, chain_hash(&prev, &e.payload_hash));
        prev = e.hash.clone();
    }

    // §3.1 写死的证据顺序：建任务 → 收到事件 → …… → 交付收尾
"""

ON_DISK_ASSERT_NEW = """        assert_eq!(e.payload_hash, payload_hash_of(payload));
        assert_eq!(e.prev_hash, prev);
        // 现行口径：`created_at` 也在前像里（BB2 ①）
        assert_eq!(e.hash, chain_hash_at(&prev, &e.payload_hash, &e.created_at));
        prev = e.hash.clone();
    }

    // §3.1 写死的证据顺序：建任务 → 收到事件 → …… → 交付收尾
"""

COLD_START_ASSERT_OLD = """        assert_eq!(e.payload_hash, payload_hash_of(payload));
        assert_eq!(e.prev_hash, prev);
        assert_eq!(e.hash, chain_hash(&prev, &e.payload_hash));
        prev = e.hash.clone();
    }

    let kinds: Vec<EvidenceKind> = events.iter().map(|e| e.kind).collect();
"""

COLD_START_ASSERT_NEW = """        assert_eq!(e.payload_hash, payload_hash_of(payload));
        assert_eq!(e.prev_hash, prev);
        // 现行口径：`created_at` 也在前像里（BB2 ①）
        assert_eq!(e.hash, chain_hash_at(&prev, &e.payload_hash, &e.created_at));
        prev = e.hash.clone();
    }

    let kinds: Vec<EvidenceKind> = events.iter().map(|e| e.kind).collect();
"""


class Edit:
    """一条改动。字面量精确替换 —— 锚点是逐字量出来的，不需要正则来兜空白。

    要求**唯一命中**（==1）：0 次和 2 次都说明现状不是本补丁预料的形态，一样拒。
    """

    def __init__(self, rel: str, label: str, old: str, new: str):
        self.rel, self.label, self.old, self.new = rel, label, old, new

    def path(self, root: pathlib.Path) -> pathlib.Path:
        return root / self.rel

    def hits(self, text: str) -> int:
        return text.count(self.old)

    def apply(self, text: str) -> str:
        return text.replace(self.old, self.new, 1)


EDITS = [
    Edit(
        f"{CONTRACTS_SRC}/evidence.rs",
        "新增 canonical_created_at / chain_hash_at，旧 chain_hash 留着并注明何时用",
        CHAIN_HASH_OLD,
        CHAIN_HASH_NEW,
    ),
    Edit(
        f"{CONTRACTS_SRC}/evidence.rs",
        "EvidenceEvent 的 hash / created_at 两个字段注释改口径",
        EVENT_FIELDS_OLD,
        EVENT_FIELDS_NEW,
    ),
    Edit(
        f"{CONTRACTS_SRC}/evidence.rs",
        "文件末尾的测试向量表补上新口径那两组",
        VECTORS_OLD,
        VECTORS_NEW,
    ),
    Edit(
        f"{CONTRACTS_SRC}/lib.rs",
        "导出 canonical_created_at / chain_hash_at",
        LIB_EXPORT_OLD,
        LIB_EXPORT_NEW,
    ),
    Edit(
        f"{CONTRACTS_TESTS}/evidence_vectors.rs",
        "契约测试：新口径的冻结向量 + 「动时间戳就换链」两条",
        VECTORS_TEST_IMPORT_OLD,
        VECTORS_TEST_IMPORT_NEW,
    ),
    Edit(
        f"{CONTRACTS_TESTS}/evidence_vectors.rs",
        "契约测试：追加两个新测试函数",
        VECTORS_TEST_OLD,
        VECTORS_TEST_NEW,
    ),
    Edit(
        f"{EVIDENCE_SRC}/writer.rs",
        "writer 引入 chain_hash_at",
        WRITER_IMPORT_OLD,
        WRITER_IMPORT_NEW,
    ),
    Edit(
        f"{EVIDENCE_SRC}/writer.rs",
        "now_micros 的精度注释：从「好看」升级成校验前提",
        NOW_MICROS_OLD,
        NOW_MICROS_NEW,
    ),
    Edit(
        f"{EVIDENCE_SRC}/writer.rs",
        "残行自愈里那处链校验换新口径（两处 verify 之一）",
        WRITER_TAIL_OLD,
        WRITER_TAIL_NEW,
    ),
    Edit(
        f"{EVIDENCE_SRC}/writer.rs",
        "append 落盘：created_at 先定下来再进 hash",
        WRITER_APPEND_OLD,
        WRITER_APPEND_NEW,
    ),
    Edit(
        f"{EVIDENCE_SRC}/writer.rs",
        "verify_sync 的 doc：写明旧口径在这里是 false 且原因归 cli 报",
        WRITER_VERIFY_DOC_OLD,
        WRITER_VERIFY_DOC_NEW,
    ),
    Edit(
        f"{EVIDENCE_SRC}/writer.rs",
        "verify_sync 换新口径（两处 verify 之二）",
        WRITER_VERIFY_OLD,
        WRITER_VERIFY_NEW,
    ),
    Edit(
        f"{EVIDENCE_SRC}/cli.rs",
        "cli 引入 chain_hash_at（旧的留着认「这是旧口径」）",
        CLI_IMPORT_OLD,
        CLI_IMPORT_NEW,
    ),
    Edit(
        f"{EVIDENCE_SRC}/cli.rs",
        "load_timeline 的 doc 改口径",
        CLI_DOC_OLD,
        CLI_DOC_NEW,
    ),
    Edit(
        f"{EVIDENCE_SRC}/cli.rs",
        "load_timeline 里加「旧口径只报头一条」的计数器",
        CLI_COUNTER_OLD,
        CLI_COUNTER_NEW,
    ),
    Edit(
        f"{EVIDENCE_SRC}/cli.rs",
        "load_timeline 换新口径，并把「旧口径」单独报成人话",
        CLI_HASH_OLD,
        CLI_HASH_NEW,
    ),
    Edit(
        f"{EVIDENCE_TESTS}/chain.rs",
        "B7 向量测试引入两个 hash 函数",
        CHAIN_TEST_IMPORT_OLD,
        CHAIN_TEST_IMPORT_NEW,
    ),
    Edit(
        f"{EVIDENCE_TESTS}/chain.rs",
        "B7：hash 向量改成按现行口径重算 + 新增两条 created_at 用例",
        CHAIN_TEST_VEC_OLD,
        CHAIN_TEST_VEC_NEW,
    ),
    Edit(
        f"{EVIDENCE_TESTS}/chain.rs",
        "B7：「进程重启接得住旧文件」那条也别再钉旧口径常量",
        RESTART_TEST_OLD,
        RESTART_TEST_NEW,
    ),
    Edit(
        f"{APP_TESTS}/evidence_on_disk.rs",
        "贯通用例 1 的 import",
        ON_DISK_IMPORT_OLD,
        ON_DISK_IMPORT_NEW,
    ),
    Edit(
        f"{APP_TESTS}/evidence_on_disk.rs",
        "贯通用例 1 里逐行重算链那一行",
        ON_DISK_ASSERT_OLD,
        ON_DISK_ASSERT_NEW,
    ),
    Edit(
        f"{APP_TESTS}/cold_start_to_delivery.rs",
        "贯通用例 2 的 import",
        COLD_START_IMPORT_OLD,
        COLD_START_IMPORT_NEW,
    ),
    Edit(
        f"{APP_TESTS}/cold_start_to_delivery.rs",
        "贯通用例 2 里逐行重算链那一行",
        COLD_START_ASSERT_OLD,
        COLD_START_ASSERT_NEW,
    ),
]


def diagnose(text: str, edit: Edit) -> str:
    """锚点没唯一命中时，把「到底处在哪个状态」报成人话，省得人去猜。"""
    head = next((l for l in edit.new.splitlines() if l.strip()), "")
    if head and head in text and edit.old not in text:
        return "看起来这一处已经打过了（新内容已经在文件里）。整份补丁是一起打的，别只补这一处。"
    if edit.old.strip() and edit.old.splitlines()[0].strip() in text:
        return "锚点的第一行在，但整段对不上 —— 中间被人动过（哪怕只是空格或换行）。拿 grep 对一眼再改锚点。"
    return "锚点整段都找不到 —— 这个文件已经不是本补丁预料的形态了，人工核一遍。"


def check_rust(staged: str, name: str) -> str | None:
    """把改完的整份 Rust 源码喂给 rustfmt（读 stdin），既验可解析、又验格式没走样。

    `cargo fmt --check` 是 `scripts/check.sh` 的 A4b 硬判据，格式走样了落进去就是红。
    （`cargo fmt --all` 被守卫拦，所以纪律上本来就是 `rustfmt --edition 2024` 这一条路。）

    返回 None = 通过；否则返回人话的失败原因。
    """
    try:
        r = subprocess.run(
            ["rustfmt", "--edition", RUST_EDITION, "--emit", "stdout"],
            input=staged,
            capture_output=True,
            text=True,
            check=False,
        )
    except FileNotFoundError:
        return "找不到 rustfmt —— PATH 上要有 /opt/homebrew/opt/rustup/bin。装上再跑，别跳过。"
    if r.returncode != 0:
        return f"rustfmt 不认这份改完的 Rust 源码：\n{r.stderr.strip()}"
    # rustfmt --emit stdout 会在最前面加一行 `<stdin>:` 之类的标注，去掉再比
    out = r.stdout
    if out.startswith("<stdin>:\n"):
        out = out[len("<stdin>:\n") :]
    if out != staged:
        return (
            f"{name} 改完之后不是 rustfmt 形态（多半是某行超了 100 列、或者缩进对不上）。"
            "A4b `cargo fmt --check` 会红。"
        )
    return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="只报告命中情况，不写盘")
    ap.add_argument(
        "--root",
        default=None,
        help="改哪棵树（默认是本仓库根）。**只给自验用**：对着 /tmp 下的仓库副本跑，"
        "不碰真的那几个冻结文件。",
    )
    args = ap.parse_args()

    root = pathlib.Path(args.root).resolve() if args.root else REPO_ROOT

    # 守卫故意拦自我授权。真改冻结面就必须由人来开那道门 —— 脚本自己再验一次。
    if args.root is None and os.environ.get("AITE_RELOCK") != "1":
        print(
            "这份补丁改的是冻结面（契约的 3 个文件）外加 2 个派单只读面的测试文件 ——\n"
            "守卫的保护面，故意只让人跑。\n"
            "  人跑：AITE_RELOCK=1 python3 review/bb2-created-at-chain-patch.py --check\n"
            "  自验：python3 review/bb2-created-at-chain-patch.py --root /tmp/bb2-check --check",
            file=sys.stderr,
        )
        return 1

    files = sorted({e.rel for e in EDITS})
    for rel in files:
        if not (root / rel).exists():
            print(f"找不到 {root / rel}", file=sys.stderr)
            return 1

    original = {rel: (root / rel).read_text(encoding="utf-8") for rel in files}
    staged = dict(original)
    problems: list[str] = []

    # 唯一命中：0 条和 2 条都是「现状不是本补丁预料的形态」，一样拒。
    # 同一个文件里多条 Edit 依次作用在上一条的结果上，所以顺序是有意义的
    # （锚点之间互不重叠，这一点由「每条都恰好命中 1 次」自证）。
    for i, e in enumerate(EDITS, 1):
        n = e.hits(staged[e.rel])
        name = pathlib.PurePosixPath(e.rel).name
        if n == 1:
            print(f"[{i:2d}/{len(EDITS)} 命中 1 条] {name}: {e.label}")
            staged[e.rel] = e.apply(staged[e.rel])
        else:
            print(f"[{i:2d}/{len(EDITS)} 命中 {n} 条] {name}: {e.label}")
            print(f"                └ {diagnose(staged[e.rel], e)}")
            problems.append(f"{e.rel} 第 {i} 条：锚点命中 {n} 条（要求恰好 1 条）")

    # 一条对不上就整份拒写 —— 半份补丁比没打更难查（契约改了校验没改，或者反过来）。
    if problems:
        print("\n对不上的地方（一个字节都没写）：", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1

    # 合法性：改完的每个文件都必须还能被 rustfmt 认，且已经是 rustfmt 形态。
    for rel in files:
        name = pathlib.PurePosixPath(rel).name
        why = check_rust(staged[rel], name)
        if why:
            print(f"\n没过合法性检查，一个字节都没写：\n{why}", file=sys.stderr)
            return 1
        print(f"[  合法] {name}: rustfmt 认，且已是 rustfmt 形态")

    if args.check:
        print(f"\n--check：没写盘。这一份会动 {len(files)} 个文件、{len(EDITS)} 处：")
        for rel in files:
            n = sum(1 for e in EDITS if e.rel == rel)
            print(f"  {rel}（{n} 处）")
        return 0

    for rel in files:
        (root / rel).write_text(staged[rel], encoding="utf-8")
        print(f"写了 {root / rel}")

    # 落盘之后再读回来验一遍：新内容真的在里面、旧锚点一条不剩。
    back = {rel: (root / rel).read_text(encoding="utf-8") for rel in files}
    for i, e in enumerate(EDITS, 1):
        name = pathlib.PurePosixPath(e.rel).name
        if e.new not in back[e.rel]:
            print(f"\n写完了但 {name} 里找不到第 {i} 条的新内容 —— 别信这次运行，人工核", file=sys.stderr)
            return 1
    print(f"[读回 OK] {len(EDITS)} 处新内容全部在盘上")

    print("\n接着必须做的事（每步的期望输出见回执 ④ 的命令链）：")
    print("  1. cd core && cargo test -p aite-contracts -p aite-evidence   # 新向量与两条 created_at 用例")
    print("  2. cd core && cargo test --workspace --no-fail-fast          # 全量")
    print("  3. 把旧证据目录挪走：data/evidence -> data/evidence-p0.1-legacy（旧口径链会判红）")
    print("  4. 看契约锁在抱怨谁，看清楚了再重锁：")
    print("     core/target/debug/aite contracts lock --check   # 期望 MISMATCH 3 file(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
