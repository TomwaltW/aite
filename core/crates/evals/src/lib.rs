//! aite-evals —— 评测 runner / 场景 / checks / protocol probe / demo-fixture（对应旧 aite/evals/** 与 scripts/demo_fixture.py）。owner: R7
//!
//! R0 只放骨架。cli::run 是 aite 二进制的子命令入口：本轨落地前打印 not implemented 并返回 2。
pub mod cli {
    /// 子命令入口：收到的是子命令之后的全部参数（本 crate 自己解析）。返回进程退出码。
    pub fn run(args: Vec<String>) -> i32 {
        eprintln!("not implemented: aite-evals（args={args:?}）");
        2
    }
}
