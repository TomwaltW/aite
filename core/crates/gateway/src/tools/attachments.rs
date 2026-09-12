//! `download_attachment`（对应旧 `aite/tools/attachments.py`）—— 把本次消息的附件下到沙箱 `/work/in/`。
//!
//! 两段两种错误码，别混（§3.3）：
//! - 从平台下载失败（含「压根没有附件上下文」）→ upstream。
//!   §3.3 原文：「群里发的附件 adapter 下载失败 → `download_attachment` 工具返回 upstream」。
//! - 下来了但写不进沙箱 → sandbox。
//!
//! 文件名只从 `file_key` 推。§3.1 冻结的 schema 里只有 `file_key` 一个参数，拿不到原始
//! 文件名；而 file_key 是平台给的不透明串，直接当路径用会带上 `/`、`..` 之类，
//! 所以先过一遍白名单清洗（`_safe_name` 五步，逐步对齐旧实现）。
//! 容器被 reaper 收走那一条（见 `python_exec` 头注释第 3 条）在这里也成立：blob 已经
//! 从平台下来了，写不进去只是因为记账里那个 id 已经没了 —— 摘掉、重建、再写一次。
use aite_contracts::{SandboxErrorKind, ToolContext};
use serde_json::{Map, Value, json};

use super::{ToolEnv, ToolFailure, ToolOutcome, as_map, rebuild_sandbox, str_arg};

const MAX_NAME: usize = 120;
const INBOX: &str = "/work/in";

/// 只留字母数字、点、下划线、连字符和汉字，其余一律折成下划线
/// （旧实现的正则 `[^0-9A-Za-z._一-鿿-]+`，连续多个折成一个）。
fn safe_name(file_key: &str) -> String {
    // ① PurePosixPath(file_key).name（空则原串）
    let base = posix_name(file_key);
    let base = if base.is_empty() { file_key } else { base };

    // ② 白名单之外折成 _
    let mut folded = String::with_capacity(base.len());
    let mut in_run = false;
    for ch in base.chars() {
        if is_safe_char(ch) {
            folded.push(ch);
            in_run = false;
        } else if !in_run {
            folded.push('_');
            in_run = true;
        }
    }

    // ③ 去掉首尾的 . 和 _
    let stripped = folded.trim_matches(|c| c == '.' || c == '_');

    // ④ 空则 attachment，再截到 120 个字符
    let name = if stripped.is_empty() {
        "attachment"
    } else {
        stripped
    };
    name.chars().take(MAX_NAME).collect()
}

fn is_safe_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') || ('一'..='鿿').contains(&ch)
}

/// `PurePosixPath(s).name`：按 `/` 切开，**丢掉空分量和 `.`**，保留 `..`，取最后一个；
/// 一个都不剩就是空串。
///
/// 别写成「砍掉尾部斜杠再 rsplit」—— 那样 `a/b/.` 会拿到 `"."`，折完 strip 完变成空，
/// 最后落成 `attachment`；而 Python 拿到的是 `b`。file_key 是平台给的不透明串，
/// 真出现这种结尾时两边就会把同一个附件落到不同的文件名上。
fn posix_name(file_key: &str) -> &str {
    file_key
        .rsplit('/')
        .find(|part| !part.is_empty() && *part != ".")
        .unwrap_or("")
}

pub async fn download_attachment(
    env: ToolEnv,
    ctx: ToolContext,
    args: Map<String, Value>,
) -> Result<ToolOutcome, ToolFailure> {
    let platform = env.platform()?;
    let sandbox = env.sandbox()?;
    let file_key = str_arg(&args, "file_key").to_string();

    let Some(message_id) = ctx
        .attachments_message_id
        .as_deref()
        .filter(|m| !m.is_empty())
    else {
        return Err(ToolFailure::upstream(
            "本次消息没有附件（attachments_message_id 为空），没有东西可下载",
        ));
    };

    let blob = platform
        .download_file(message_id, &file_key)
        .await
        .map_err(|e| ToolFailure::upstream(format!("下载附件失败（{file_key}）：{e}")))?;

    // ⑤ /work/in/<清洗后的名字>
    let path = format!("{INBOX}/{}", safe_name(&file_key));
    let write = async {
        let sandbox_id = env.acquire_sandbox().await?;
        match sandbox.put_file(&sandbox_id, &path, &blob).await {
            Ok(()) => Ok(()),
            // 容器已经不在了：摘掉记账、重建一个再写。**只重试这一次**，
            // 重建之后再撞就照常落成 code=sandbox（§3.3 的连续沙箱失败闸不能架空）。
            Err(e) if e.kind == SandboxErrorKind::NotFound => {
                tracing::warn!(
                    sandbox_id = %sandbox_id,
                    error = %e,
                    "gateway.sandbox_gone 容器已不在，重建后重写一次"
                );
                let fresh = rebuild_sandbox(&env).await?;
                sandbox
                    .put_file(&fresh, &path, &blob)
                    .await
                    .map_err(|e| ToolFailure::sandbox(e.to_string()))
            }
            Err(e) => Err(ToolFailure::sandbox(e.to_string())),
        }
    };
    write
        .await
        .map_err(|e| ToolFailure::sandbox(format!("把附件写进沙箱失败（{path}）：{e}")))?;

    Ok(
        ToolOutcome::new(format!("附件已下载到沙箱：{path}（{} 字节）", blob.len())).with_data(
            as_map(json!({
                "path": path,
                "size": blob.len(),
                "file_key": file_key,
            })),
        ),
    )
}

#[cfg(test)]
mod safe_name_tests {
    use super::{MAX_NAME, safe_name};

    /// ① `PurePosixPath(file_key).name`：丢空分量与 `.`，保留 `..`。
    #[test]
    fn step1_takes_the_posix_basename() {
        assert_eq!(safe_name("a/b/sales.csv"), "sales.csv");
        assert_eq!(safe_name("a/b//"), "b");
        assert_eq!(safe_name("a/./b"), "b");
    }

    /// ①的分歧点（RΩ 修，审核记账 R6）：`file_key` 以 `/.` 结尾时，
    /// pathlib 丢掉那个 `.` 分量拿到 `b`；旧写法「砍尾斜杠再 rsplit」拿到 `"."`，
    /// 折完 strip 完变成空，最后落成 `attachment` —— 同一个附件两边落到不同文件名。
    #[test]
    fn step1_handles_a_trailing_dot_component_like_pathlib() {
        assert_eq!(safe_name("a/b/."), "b");
        assert_eq!(safe_name("a/b/./."), "b");
        // `..` 是保留的（pathlib 不折叠它），折 + strip 之后才落到 attachment
        assert_eq!(safe_name("a/b/.."), "attachment");
    }

    /// ② 白名单之外折成 `_`，**连续多个折成一个**。
    #[test]
    fn step2_folds_unsafe_runs_into_a_single_underscore() {
        assert_eq!(safe_name("a b.csv"), "a_b.csv");
        assert_eq!(safe_name("a???b.csv"), "a_b.csv");
        // 汉字在白名单里，原样留着
        assert_eq!(safe_name("销售 数据.csv"), "销售_数据.csv");
        // 路径分隔与爬升都被折掉（这就是「不能直接拿 file_key 当路径」的理由）
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
    }

    /// ③ 去掉首尾的 `.` 与 `_`（中间的不动）。
    #[test]
    fn step3_strips_leading_and_trailing_dots_and_underscores() {
        assert_eq!(safe_name(".hidden"), "hidden");
        assert_eq!(safe_name("__name__"), "name");
        assert_eq!(safe_name("...a.b..."), "a.b");
        assert_eq!(safe_name("a.b"), "a.b", "中间的点不许动");
    }

    /// ④ 空则 `attachment`。
    #[test]
    fn step4_falls_back_to_attachment_when_nothing_is_left() {
        for key in ["", ".", "/", "/.", "...", "___", "???"] {
            assert_eq!(safe_name(key), "attachment", "file_key={key:?}");
        }
    }

    /// ④ 再截到 120 个**字符**（不是字节）。
    #[test]
    fn step4_clips_to_120_chars_not_bytes() {
        let long = "a".repeat(500);
        assert_eq!(safe_name(&long).len(), MAX_NAME);

        // 汉字一个 3 字节：按字节截会切在 rune 中间
        let cn = "数".repeat(500);
        let got = safe_name(&cn);
        assert_eq!(got.chars().count(), MAX_NAME);
        assert!(!got.contains('\u{fffd}'), "切在了 char 边界里：{got}");
    }

    /// 真实形状的 file_key 原样过（飞书的是不透明串，不该被改）。
    #[test]
    fn a_real_looking_file_key_passes_through() {
        assert_eq!(
            safe_name("file_v3_00d1_abc-123.XYZ"),
            "file_v3_00d1_abc-123.XYZ"
        );
    }
}
