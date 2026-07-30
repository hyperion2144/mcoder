// 终审修复 #2: spawn_run_loop wrapper 与新 loop 竞态防护
//
// 单调递增的 generation token；
// 旧 loop 在执行清理动作前必须 compare entry.generation 与 my_gen：
// - 相等：自己是当前 loop，可执行清理
// - 不等：自己已被新 loop 接管，立即短路
//
// 这是比单纯 loop_running CAS 更强的 fencing：
// - CAS 只防"同时 spawn"（同 generation 谁能 CAS 到 0→1）
// - generation 防止"旧 loop 还在写 loop_state，新 loop 已接管"

use std::sync::atomic::{AtomicU64, Ordering};

/// 轻量级 fencing token（被 SessionEntry.generation 直接 hold）。
/// 提取出来供 unit test 覆盖核心语义（单线程 + 并发）。
pub struct GenerationFence {
    inner: AtomicU64,
}

impl GenerationFence {
    pub fn new() -> Self {
        Self {
            inner: AtomicU64::new(0),
        }
    }

    /// 读取当前 generation（debug 用）
    pub fn current(&self) -> u64 {
        self.inner.load(Ordering::SeqCst)
    }

    /// spawn 前调用一次：原子地 +1，返回自己的 my_gen
    pub fn next_spawn(&self) -> u64 {
        self.inner.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// 是否仍为当前 generation（清理动作前调用）
    pub fn is_still_current(&self, my_gen: u64) -> bool {
        self.inner.load(Ordering::SeqCst) == my_gen
    }
}

impl Default for GenerationFence {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_is_zero() {
        let f = GenerationFence::new();
        assert_eq!(f.current(), 0);
    }

    #[test]
    fn sequential_bumps() {
        let f = GenerationFence::new();
        assert_eq!(f.next_spawn(), 1);
        assert_eq!(f.next_spawn(), 2);
        assert_eq!(f.next_spawn(), 3);
        assert_eq!(f.current(), 3);
    }

    #[test]
    fn is_still_current_true_for_my_gen() {
        let f = GenerationFence::new();
        let my = f.next_spawn();
        assert!(f.is_still_current(my));
        assert!(!f.is_still_current(my + 99));
    }
}
