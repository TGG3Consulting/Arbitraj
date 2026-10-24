//! Файл: tests/bsc_dex_heads_poll.rs
//! Что делает: решение 57 — номер блока опросом eth_blockNumber вместо подписки newHeads. На живых данных 2026-10-24
//! (tests/data/live_2026-10-24/heads.json, канал heads сырого журнала узла): новые номера блоков идут слушателям, тот
//! же или меньший номер — нет (свежесть не продлевается, пока узел стоит); цена до и после по таблице NodeReal
//! (https://docs.nodereal.io/docs/compute-units-cus: WS — 0,04 CU/байт, eth_blockNumber — 5 CU).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::node::heads. Моков нет.

use serde_json::Value;

use arb::venues::bsc_dex::node::heads::Heads;

fn fixture() -> Value {
    let raw = std::fs::read_to_string("tests/data/live_2026-10-24/heads.json").unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn only_growing_block_numbers_are_new_heads() {
    let fx = fixture();
    let heads = Heads::default();
    let mut rx = heads.watch();
    let nums: Vec<u64> = fx["first"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x[1].as_u64().unwrap())
        .collect();
    for (i, n) in nums.iter().enumerate() {
        assert!(heads.observe(*n), "блок {n} — новый");
        assert!(rx.has_changed().unwrap());
        rx.borrow_and_update();
        // Опрос чаще блоков: тот же номер ещё раз — не новая голова, свежесть не продлевается.
        let at = heads.latest().unwrap().received_at;
        assert!(!heads.observe(*n));
        assert_eq!(heads.latest().unwrap().received_at, at);
        // Узел за балансировщиком ответил прошлым номером — назад не идём.
        if i > 0 {
            assert!(!heads.observe(nums[i - 1]));
        }
        assert!(!rx.has_changed().unwrap());
    }
    assert_eq!(heads.latest().unwrap().number, *nums.last().unwrap());
}

#[test]
fn polling_costs_far_less_than_new_heads() {
    let fx = fixture();
    let secs = fx["window_s"].as_f64().unwrap();
    let bytes = fx["heads_bytes"].as_f64().unwrap();
    let blocks = fx["heads_notifications"].as_f64().unwrap();
    let ws_cu_per_s = bytes / secs * 0.04;
    // Опрос раз в блок (самый частый осмысленный интервал по этой записи) — 5 CU за вызов.
    let poll_cu_per_s = blocks / secs * 5.0;
    assert!(ws_cu_per_s > 200.0, "newHeads: {ws_cu_per_s} CU/с");
    assert!(poll_cu_per_s < 12.0, "опрос: {poll_cu_per_s} CU/с");
}
