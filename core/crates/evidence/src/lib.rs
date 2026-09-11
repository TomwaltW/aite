//! aite-evidence —— EvidenceWriter 的文件实现 + aite evidence show（对应旧 aite/evidence/writer.py 与 scripts/evidence_show.py）。owner: R3
//!
//! R0 只放骨架。cli::run 是 aite 二进制的子命令入口：本轨落地前打印 not implemented 并返回 2。
pub mod cli {
    /// 子命令入口：收到的是子命令之后的全部参数（本 crate 自己解析）。返回进程退出码。
    pub fn run(args: Vec<String>) -> i32 {
        eprintln!("not implemented: aite-evidence（args={args:?}）");
        2
    }
}
