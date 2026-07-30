// 终审修复 #3: todo gate 每 loop 最多合理 strikes (3 次)
// 第 3 次仍未完成 → 结构化提醒 + 结束，不自动 cancel todos。
//
// 旧实现：仅 1 strike 后直接放行结束（无变化则结束）。
// 新实现：可配置 MAX_STRIKES=3，前两次 continue 注入不同强度的提醒，
//         第 3 次注入"finishing"提醒后放行结束。
//
// 设计动机：避免模型在第一次提醒后又原地踏步、循环浪费 token。
// strikes 用 fingerprint（status|priority|content 拼接）+ strike 计数同时跟踪。
// - fingerprint 不变 → strike++
// - fingerprint 变了 → strike reset (= 1，下次计数从 1 起)
//
// 该函数是纯逻辑，单元测试可独立覆盖。

use crate::persistence::session_state::TodoRecord;

pub const MAX_STRIKES: u32 = 3;

/// todo gate 决策
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoGateDecision {
    /// 注入 [unfinished todos] 提醒后继续循环（strike 1, 2）
    Continue { strike: u32, message: String },
    /// 第 3 次仍未变化 → 注入 finishing 提醒 + 结束 loop（结构化、不 cancel todos）
    FinishWithReminder { strike: u32, message: String },
    /// 没有未完成 → 正常结束
    Finish,
}

/// 计算 fingerprint（status|priority|content 拼接）
pub fn fingerprint(items: &[TodoRecord]) -> String {
    if items.is_empty() {
        return String::new();
    }
    items
        .iter()
        .map(|t| format!("{}|{}|{}", t.status, t.priority, t.content))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 纯函数：todo gate 决策
///
/// 输入：
/// - `items`：当前 todo 列表（pending + in_progress）
/// - `last_fingerprint`：上一次观察到 fingerprint（None 表示首次观察）
/// - `last_strike`：上一次 strike 计数（None=未知/初值=0）
///
/// 输出：
/// - Finish：无未完成
/// - Continue { strike, message }：指纹变了 OR (指纹相同但 strike < MAX_STRIKES)
/// - FinishWithReminder { strike, message }：指纹相同且 strike == MAX_STRIKES
///
/// **关键不变量**：
/// - 指纹变了 → strike 重置为 1（即使是 last_strike==MAX_STRIKES）
/// - 指纹相同 → strike 累加
pub fn decide_todo_gate(
    items: &[TodoRecord],
    last_fingerprint: Option<&str>,
    last_strike: Option<u32>,
) -> TodoGateDecision {
    if items.is_empty() {
        return TodoGateDecision::Finish;
    }
    let fp = fingerprint(items);
    let lines: Vec<String> = items
        .iter()
        .map(|t| format!("- [{}] {} ({})", t.status, t.content, t.priority))
        .collect();
    let joined = lines.join("\n");

    let last = last_fingerprint.unwrap_or("");
    if last != fp {
        // 指纹变了：重置为 1，注入提醒，continue
        let message = format!(
            "[unfinished todos] you have {} unfinished todo(s); \
             continue working until all are completed or cancelled:\n{}",
            items.len(),
            joined
        );
        return TodoGateDecision::Continue {
            strike: 1,
            message,
        };
    }

    // 指纹相同：累加 strike
    let prev = last_strike.unwrap_or(0);
    let next_strike = prev + 1;
    if next_strike >= MAX_STRIKES {
        // 已达上限：finish with structured reminder；不 cancel todos
        let message = format!(
            "[system finishing reminder] you have stopped with the following {} \
             unfinished todo(s) for {} strikes; the loop will end now without auto-cancelling them. \
             Manually complete or cancel them before resuming:\n{}",
            items.len(),
            prev.max(1),
            joined
        );
        return TodoGateDecision::FinishWithReminder {
            strike: next_strike,
            message,
        };
    }
    // 还没到上限
    let message = format!(
        "[unfinished todos (strike {})] you still have {} unfinished todo(s); \
         change their status (complete/cancel) or finish the work to end the loop:\n{}",
        next_strike,
        items.len(),
        joined
    );
    TodoGateDecision::Continue {
        strike: next_strike,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::session_state::{TodoRecord, PRIORITY_MEDIUM, STATUS_PENDING};

    fn todo_item(content: &str) -> TodoRecord {
        TodoRecord {
            id: format!("t-{}", content),
            session_id: "s".into(),
            content: content.into(),
            status: STATUS_PENDING.into(),
            priority: PRIORITY_MEDIUM.into(),
            order: 0,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn empty_items_returns_finish() {
        let d = decide_todo_gate(&[], None, None);
        assert_eq!(d, TodoGateDecision::Finish);
    }

    #[test]
    fn first_observation_no_last_fingerprint_returns_continue_strike_1() {
        let items = vec![todo_item("a")];
        let d = decide_todo_gate(&items, None, None);
        match d {
            TodoGateDecision::Continue { strike, .. } => assert_eq!(strike, 1),
            _ => panic!("expected Continue strike=1"),
        }
    }

    #[test]
    fn changed_fingerprint_resets_strike_to_1() {
        let items = vec![todo_item("a")];
        // 即使 last_strike=99，只要 fingerprint 变了，strike 重置为 1
        let d = decide_todo_gate(&items, Some("different-fp"), Some(99));
        match d {
            TodoGateDecision::Continue { strike, .. } => assert_eq!(strike, 1),
            _ => panic!("changed fingerprint must reset strike to 1"),
        }
    }

    #[test]
    fn same_fingerprint_increments_strike() {
        let items = vec![todo_item("a")];
        let fp = fingerprint(&items);
        let d = decide_todo_gate(&items, Some(&fp), Some(1));
        match d {
            TodoGateDecision::Continue { strike, .. } => assert_eq!(strike, 2),
            _ => panic!("expected Continue strike=2"),
        }
    }

    #[test]
    fn strike_reaching_max_returns_finish_with_reminder() {
        let items = vec![todo_item("a")];
        let fp = fingerprint(&items);
        // strike 累加到 3 时触发 FinishWithReminder
        let d = decide_todo_gate(&items, Some(&fp), Some(2));
        match d {
            TodoGateDecision::FinishWithReminder { strike, .. } => {
                assert_eq!(strike, MAX_STRIKES);
            }
            _ => panic!("expected FinishWithReminder after MAX_STRIKES"),
        }
    }

    #[test]
    fn finish_reminder_does_not_cancel_todos_internally() {
        // 该函数本身不操作 todo；确保行为只产出 message。
        let items = vec![todo_item("a"), todo_item("b")];
        let fp = fingerprint(&items);
        let d = decide_todo_gate(&items, Some(&fp), Some(2));
        match d {
            TodoGateDecision::FinishWithReminder { message, .. } => {
                // 不应出现"已取消"语义
                assert!(
                    !message.contains("cancelled"),
                    "the message must not auto-cancel, but got: {}",
                    message
                );
                assert!(message.contains("2 unfinished"));
                assert!(message.contains("strike"));
            }
            _ => panic!("expected FinishWithReminder"),
        }
    }
}
