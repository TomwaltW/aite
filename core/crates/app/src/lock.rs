//! 契约锁：记录并校验契约文件的 sha256（对应旧 aite/contracts/lock.py）。
//!
//!   aite contracts lock --write    # 生成 / 刷新仓库根的 .contracts.lock
//!   aite contracts lock --check    # 一致 -> "OK <n> files" 退出 0；否则打印差异退出 1
//!
//! 锁定面：proto/aite/v1/**、core/crates/contracts/**（Cargo.toml + src + tests）、aite/contracts/**（Python，RΩ 删除时一并重锁）。
//! 收目录下**所有**文件而不只是源码：往契约目录丢任何东西都会让 --check 变红，保护面没有洞。
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

pub const LOCK_FILE: &str = ".contracts.lock";
pub const LOCK_DIRS: &[&str] = &["proto/aite/v1", "core/crates/contracts", "aite/contracts"];
const HEADER: &str = "# aite contracts lock — sha256 of proto/aite/v1/**, core/crates/contracts/**, aite/contracts/**（lock.py/__pycache__/target 除外）";
/// 五份 proto + contracts crate（Cargo.toml + 13 源 + 7 测试）+ Python 11 份；少于这个数一定是出事了
pub const MIN_CONTRACT_FILES: usize = 20;

fn skip(rel: &Path) -> bool {
    rel.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s == "__pycache__" || s == "target" || s == ".DS_Store"
    }) || rel.ends_with("aite/contracts/lock.py")
}

pub fn find_repo_root(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return p
            .canonicalize()
            .with_context(|| format!("--repo {} 不存在", p.display()));
    }
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join("proto/aite/v1").is_dir() && dir.join("core/Cargo.toml").is_file() {
            return Ok(dir);
        }
        if !dir.pop() {
            bail!(
                "从当前目录向上找不到仓库根（要有 proto/aite/v1 与 core/Cargo.toml）；可用 --repo 指定"
            );
        }
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("读目录 {}", dir.display()))? {
        let p = entry?.path();
        if p.is_dir() {
            walk(&p, out)?;
        } else if p.is_file() {
            out.push(p);
        }
    }
    Ok(())
}

pub fn current(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = Vec::new();
    for d in LOCK_DIRS {
        let dir = root.join(d);
        if dir.is_dir() {
            walk(&dir, &mut files)?;
        }
    }
    let mut out = BTreeMap::new();
    for p in files {
        let rel = p.strip_prefix(root)?.to_path_buf();
        if skip(&rel) {
            continue;
        }
        let bytes = fs::read(&p)?;
        out.insert(
            rel.to_string_lossy().replace('\\', "/"),
            hex::encode(Sha256::digest(&bytes)),
        );
    }
    Ok(out)
}

pub fn render(entries: &BTreeMap<String, String>) -> String {
    let mut s = String::from(HEADER);
    s.push('\n');
    for (rel, h) in entries {
        s.push_str(&format!("{h}  {rel}\n"));
    }
    s
}

pub fn parse(text: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (h, rel) = line
            .split_once("  ")
            .with_context(|| format!("无法解析的锁行：{line:?}"))?;
        out.insert(rel.trim().to_string(), h.trim().to_string());
    }
    Ok(out)
}

fn guard_not_empty(entries: &BTreeMap<String, String>) -> Result<()> {
    if entries.len() < MIN_CONTRACT_FILES {
        bail!(
            "契约文件只找到 {} 个（期望 >= {MIN_CONTRACT_FILES}）：{:?}\n锁定目录是不是被删了或仓库根找错了？",
            entries.len(),
            entries.keys().collect::<Vec<_>>()
        );
    }
    Ok(())
}

pub fn run(write: bool, repo: Option<PathBuf>) -> Result<i32> {
    let root = find_repo_root(repo)?;
    let entries = current(&root)?;
    guard_not_empty(&entries)?;
    let lock_path = root.join(LOCK_FILE);
    if write {
        if std::env::var("AITE_RELOCK").as_deref() != Ok("1") {
            bail!("--write 需要 AITE_RELOCK=1（只有总管 / R0 该重锁契约）");
        }
        fs::write(&lock_path, render(&entries))?;
        println!("wrote {} files -> {LOCK_FILE}", entries.len());
        return Ok(0);
    }
    if !lock_path.exists() {
        eprintln!("MISSING {LOCK_FILE} 不存在，先跑 --write");
        return Ok(1);
    }
    let locked = parse(&fs::read_to_string(&lock_path)?)?;
    let mut diffs = Vec::new();
    let keys: std::collections::BTreeSet<&String> = locked.keys().chain(entries.keys()).collect();
    for rel in keys {
        match (locked.get(rel), entries.get(rel)) {
            (Some(_), None) => diffs.push(format!("  deleted  {rel}（在锁里但文件不存在）")),
            (None, Some(_)) => diffs.push(format!("  added    {rel}（新文件未入锁）")),
            (Some(l), Some(a)) if l != a => {
                diffs.push(format!("  changed  {rel}\n    locked {l}\n    actual {a}"))
            }
            _ => {}
        }
    }
    if !diffs.is_empty() {
        eprintln!("MISMATCH {} file(s):", diffs.len());
        for d in diffs {
            eprintln!("{d}");
        }
        return Ok(1);
    }
    println!("OK {} files", entries.len());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_render_round_trip() {
        let mut m = BTreeMap::new();
        m.insert("proto/aite/v1/a.proto".to_string(), "ab".repeat(32));
        let text = render(&m);
        assert!(text.starts_with("# aite contracts lock"));
        assert_eq!(parse(&text).unwrap(), m);
    }

    #[test]
    fn skips_tooling_and_caches() {
        assert!(skip(Path::new("aite/contracts/lock.py")));
        assert!(skip(Path::new("aite/contracts/__pycache__/x.pyc")));
        assert!(!skip(Path::new("core/crates/contracts/src/lib.rs")));
    }
}
