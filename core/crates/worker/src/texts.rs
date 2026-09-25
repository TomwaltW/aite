//! 给人看 / 给模型看的固定文案（对应旧 aite/worker/loop.py 的模块常量 +
//! review/inventory-core.md §5 的 worker 那一段）。
//!
//! §3.3 三条硬约束之一：**逐字不变**。评测的 `text contains` 断言与 M1–M6 的验收剧本
//! 都按这些字串找东西，改一个标点就会在别处红。所以这里只放常量与格式化函数，
//! 不放判断逻辑 —— 判断在 agent.rs，文案在这里，两边分开改不动对方。

/// §3.3「只返回文本」的兜底：第一步之后再只回文本，回这句让它接着用工具。
pub const NUDGE_TEXT: &str = "请调用 final 交付结果，或调用一个工具继续。";

/// W5：final 的两种参数错误。
pub const FINAL_REPLY_REQUIRED: &str = "final.reply 必填且不能为空";
pub const FINAL_ARTIFACTS_MUST_BE_ARRAY: &str = "final.artifacts 必须是数组";

/// 同一批里 final 参数不合法时，排在它后面、没执行的那些调用的回复（CC3 ⑥）。
pub const SKIPPED_AFTER_INVALID_FINAL: &str = "同一批的 final 参数不合法，本调用未执行";

/// checklist_* 的参数错误与成功回执。
pub const CHECKLIST_ITEMS_INVALID: &str = "items 必须是 1–8 个非空字符串";
pub const CHECKLIST_REASON_REQUIRED: &str = "reason 必填";
pub const CHECKLIST_TEXT_REQUIRED: &str = "text 必填";
pub const CHECKLIST_NOTE_UPDATED: &str = "备注已更新";

/// 命中 `REPEAT_NUDGE_AT` 时回给模型的话。目标是让它**换招**，所以不说「你重复了」，
/// 而是给一个结论（这条路走不通）加两个具体的下一步。T17 里模型自己都已经诊断出
/// 「沙箱每条命令都返回空输出」了 —— 它缺的从来不是「你重复了」这个信息。
pub fn repeat_nudge(count: u32, name: &str) -> String {
    format!(
        "你已经用完全相同的参数调用了 {count} 次 {name}，每次拿到的结果都一样——这条路走不通。\
不要再原样重试：换个做法（换参数、换工具、或者换个角度拿这个信息）；\
如果确实拿不到，就调用 final，说清你卡在哪一步、手里已经有什么。"
    )
}

pub fn checklist_added(n: usize, ids: &[String]) -> String {
    format!("已添加 {n} 项：{}", ids.join(", "))
}

pub fn checklist_marked(item_id: &str, state: &str) -> String {
    format!("{item_id} 已标记为 {state}")
}

/// `f"没有这一项：{id!r}"` —— 注意是 Python 的 repr，字符串带单引号。
pub fn checklist_no_such_item(id_repr: &str) -> String {
    format!("没有这一项：{id_repr}")
}

pub fn unknown_tool(name: &str) -> String {
    format!("未知工具：{name}")
}

pub fn tool_unavailable(name: &str) -> String {
    format!("工具不可用：{name}")
}

/// Gateway 返回失败但 content 是空的时候，替模型把错误码和消息拼出来。
pub fn tool_error_content(code: &str, message: &str) -> String {
    format!("[{code}] {message}")
}

/// 署名（CC3 ④）：多人话题里模型得知道每句话是谁说的。`[名字] 正文`。
pub fn attributed_line(name: &str, text: &str) -> String {
    format!("[{name}] {text}")
}

pub fn omitted_turns(omitted: usize) -> String {
    format!("[中间省略 {omitted} 轮]")
}

// ---- 发到群里的失败面（§3.3 那张表）------------------------------------

pub fn step_limit(task_no: &str) -> String {
    format!("任务 {task_no}：已达步数上限，请缩小任务或 !new 重开")
}

pub fn wall_limit(task_no: &str) -> String {
    format!("任务 {task_no}：已达时间上限，请缩小任务或 !new 重开")
}

pub fn model_unavailable(task_no: &str) -> String {
    format!("模型服务暂不可用，任务 {task_no} 已终止")
}

pub fn repeat_failure(task_no: &str, limit: u32, tool_name: &str) -> String {
    format!(
        "任务 {task_no}：模型连续 {limit} 次原样重复调用 {tool_name}，没有进展，已终止，请换个说法或 !new 重开"
    )
}

pub fn invalid_args_failure(task_no: &str, limit: u32) -> String {
    format!("任务 {task_no}：模型连续 {limit} 次给出不合法的工具参数，已终止")
}

pub fn sandbox_failure(task_no: &str, limit: u32) -> String {
    format!("任务 {task_no}：沙箱连续 {limit} 次不可用，已终止")
}

pub fn artifact_missing(title_or_path: &str) -> String {
    format!("产物 {title_or_path} 未找到")
}

/// 助手轮的正文（CC3 ③）：发出去的那条文字，原样；有已发附件时另起一行列出标题。
/// 没有已发附件时逐字等于 `send_text` 的 `text`。DD4 依赖这个格式。
pub fn assistant_turn_content(sent_text: &str, sent_attachments: &[String]) -> String {
    if sent_attachments.is_empty() {
        return sent_text.to_string();
    }
    format!("{sent_text}\n[已发送附件] {}", sent_attachments.join("、"))
}

pub fn run_error(task_no: &str, err: &str) -> String {
    format!("任务 {task_no} 执行出错：{err}")
}
