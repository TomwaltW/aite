//! `aite` —— core 侧唯一的可执行入口。子命令一律转发到各 crate 的 cli::run，
//! 所以各轨落地时不需要动这个文件（R0 归属；要加子命令 → 停下报告）。
//!
//! RΩ 改了三处，都是"把转发接上"而不是加子命令：
//!   run / preflight  → 从 `not implemented` 换成 `aite_app::{cli, preflight}::run`
//!   evals            → 从 `cli::run(args)` 换成 `cli::run_with_wiring(args, &wiring)`，
//!                      把真 ControlPlane / 真模型 / 真沙箱的工厂注进去（§要做什么 ③）
//!
//! W3 改了一处：四个转发型子命令加 `disable_help_flag`，把 `--help` 从 clap 手里
//! 还给手写解析器 —— 理由写在 `Cmd` 的文档注释里，回归在 `tests/cli_smoke.rs`。
mod lock;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "aite", version, about = "Aite core（Rust）")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

// 子命令。
//
// **`disable_help_flag = true` 在四个转发型子命令上是必须的，别顺手删掉。**
//
// 这四个（run / evals / evidence / preflight）的参数不由 clap 解析 —— clap 只负责
// 把 argv 原样收进 `args`，真正的解析器是各 crate 自己手写的那个（`cli.rs` /
// `preflight.rs` / 两个 crate 的 `cli`）。于是 clap **不知道任何一个真实选项**，
// 它自动生成的那份 `--help` 里只有一句 `[ARGS]...`。
//
// 默认的 `--help` flag 会在参数流到手写解析器之前把它截胡，打出那份空帮助
// **并且退出码 0** —— 坏掉的帮助和好的帮助在脚本里长得一模一样，人眼也未必看得出来
// （`aite run --help` / `aite preflight --help` 从前就是这样；V6 只修得动
// `aite run -- --help` 那个写法，根治在这一层）。禁掉它之后 `--help` / `-h` 会跟着
// `allow_hyphen_values` 一起落进 `args`，由手写解析器打自己那份真用法。
//
// **边界**：这四个子命令下，`--help` / `-h` 出现在参数里**一律是求助**，加不加 `--`
// 都一样（`aite run --help` 与 `aite run -- --help` 现在打的是同一份用法）。
// 哪天某个子命令真需要把 `--help` 当业务参数透传给内层程序，就没法在这一层区分了 ——
// 那时该给那个子命令加一个显式的透传分隔符（形如 `aite run --exec -- --help`），
// **不是**把这里的 `disable_help_flag` 摘掉：摘掉就回到「帮助是空的、退出码还是 0」。
//
// `Contracts` 不在此列：它的参数是真 clap 子命令，帮助本来就带着 `lock` 与各选项。
#[derive(Subcommand)]
enum Cmd {
    /// 组装并起飞（飞书 + edge + 模型；owner RΩ）
    #[command(disable_help_flag = true)]
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// 契约锁：`aite contracts lock --check|--write`
    Contracts {
        #[command(subcommand)]
        cmd: ContractsCmd,
    },
    /// 评测：`aite evals run evals/p0 --platform fake --model scripted`（owner R7）
    #[command(disable_help_flag = true)]
    Evals {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// 证据链：`aite evidence show <task_id>`（owner R3）
    #[command(disable_help_flag = true)]
    Evidence {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// 起飞前自检（对应旧 scripts/preflight.py；owner RΩ）
    #[command(disable_help_flag = true)]
    Preflight {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ContractsCmd {
    Lock {
        /// 校验 .contracts.lock 与实际文件一致
        #[arg(long, conflicts_with = "write")]
        check: bool,
        /// 生成 / 刷新 .contracts.lock（需要 AITE_RELOCK=1，守卫也只在这时放行）
        #[arg(long)]
        write: bool,
        /// 仓库根；默认从当前目录向上找
        #[arg(long)]
        repo: Option<std::path::PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.cmd {
        Cmd::Run { args } => aite_app::cli::run(args),
        Cmd::Contracts {
            cmd: ContractsCmd::Lock { check, write, repo },
        } => {
            if !check && !write {
                eprintln!("用法：aite contracts lock --check | --write");
                2
            } else {
                match lock::run(write, repo) {
                    Ok(code) => code,
                    Err(e) => {
                        eprintln!("{e}");
                        2
                    }
                }
            }
        }
        Cmd::Evals { args } => match aite_app::wiring::evals_wiring(&args) {
            Ok(wiring) => aite_evals::cli::run_with_wiring(args, &wiring),
            // 接线本身起不来（配置读不出来 / edge 连不上 / live 模型配置缺一样）：
            // 一行人话 + 退出码 2，与 `cli` 自己的「起不来」一个口径。
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Cmd::Evidence { args } => aite_evidence::cli::run(args),
        Cmd::Preflight { args } => aite_app::preflight::run(args),
    };
    std::process::exit(code);
}
