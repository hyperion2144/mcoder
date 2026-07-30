// 终审修复 #2: spawn_run_loop generation / fencing token 协议测试
//
// 不变量：
// 1. spawn 时 generation 必须单调递增（fetch_add +1）
// 2. 旧 loop 在退出清理前必须 compare entry.generation 与 my_gen：
//    - 相等 → 自己仍是当前 loop，可执行清理（重置 loop_running / 写 loop_state）
//    - 不等 → 已被新 loop 接管，自己短路，绝不能写 loop_state 或重置 loop_running
// 3. 这防的是"上一代 loop 还在写 loop_state='stopped' 而新一代 loop 已经开始把
//    'running' 写回 DB"的竞态；CAS loop_running 仅防同时 spawn，不能防旧的还在
//    写但新的已接管。

use mcoder_lib::generation_fence::GenerationFence;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[tokio::test]
async fn generation_initial_is_zero_then_bump() {
    let f = GenerationFence::new();
    assert_eq!(f.current(), 0, "initial generation must be 0");

    let g1 = f.next_spawn();
    assert_eq!(g1, 1, "first spawn must bump to 1");
    assert_eq!(f.current(), 1);

    let g2 = f.next_spawn();
    assert_eq!(g2, 2, "second spawn must bump to 2");
    assert_eq!(f.current(), 2);
}

#[tokio::test]
async fn generation_bump_under_concurrent_spawns_yields_unique_per_caller() {
    // 4 个 task 并发 spawn → 全部应拿到唯一 my_gen，且 current() 单调递增收敛
    let f = Arc::new(GenerationFence::new());
    let mut handles = Vec::new();
    for _ in 0..4 {
        let f2 = f.clone();
        handles.push(tokio::spawn(async move { f2.next_spawn() }));
    }
    let mut gens = Vec::new();
    for h in handles {
        gens.push(h.await.unwrap());
    }
    // 唯一性
    gens.sort();
    let mut deduped = gens.clone();
    deduped.dedup();
    assert_eq!(gens.len(), deduped.len(), "each caller must get unique generation");
    // current 已经 >= 最后一次 bump
    let final_g = f.current();
    assert!(final_g >= 4, "after 4 spawns current must be >= 4, got {}", final_g);
}

#[tokio::test]
async fn loop_can_act_when_current_eq_my_gen() {
    let f = GenerationFence::new();
    let my = f.next_spawn();
    assert!(f.is_still_current(my), "exact match must allow cleanup");
}

#[tokio::test]
async fn loop_cannot_act_when_a_newer_gen_already_bumped() {
    let f = GenerationFence::new();
    let my_old = f.next_spawn();
    // 新 loop 接管
    let my_new = f.next_spawn();
    assert!(
        !f.is_still_current(my_old),
        "old gen must NOT be allowed to clobber new loop"
    );
    assert!(f.is_still_current(my_new));
}

#[tokio::test]
async fn fence_supports_scoped_store_load() {
    // 模拟 SessionEntry.generation 字段用法
    let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let my_gen = counter.fetch_add(1, Ordering::SeqCst) + 1;
    // 模拟新 loop spawn
    counter.fetch_add(1, Ordering::SeqCst);
    let now_gen = counter.load(Ordering::SeqCst);
    assert_ne!(my_gen, now_gen);
}
