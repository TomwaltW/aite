//! `aite` —— core 侧唯一的可执行入口。子命令一律转发到各 crate 的 cli::run，
//! 所以各轨落地时不需要动这个文件（R0 归属；要加子命令 → 停下报告）。
mod lock;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "aite", version, about = "Aite core（Rust）")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 组装并起飞（飞书 + edge + 模型；owner RΩ）
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
    Evals {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// 证据链：`aite evidence show <task_id>`（owner R3）
    Evidence {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// 起飞前自检（对应旧 scripts/preflight.py；owner RΩ）
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
        Cmd::Run { args } => {
            eprintln!("not implemented: aite run（RΩ 组装）args={args:?}");
            2
        }
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
        Cmd::Evals { args } => aite_evals::cli::run(args),
        Cmd::Evidence { args } => aite_evidence::cli::run(args),
        Cmd::Preflight { args } => {
            eprintln!("not implemented: aite preflight（RΩ）args={args:?}");
            2
        }
    };
    std::process::exit(code);
}
