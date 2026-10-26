//! Файл: tests/bsc_dex_receipts_resilience.rs
//! Что делает: устойчивость ленты квитанций (решение 77) — два условия восстановления «пропуск + пересинхрон», на
//! настоящей причине живого бота (блок 129907931: «квитанции больше предела bsc.receipts_total_ms»):
//! - по блоку: bsc.receipts_block_max_tries «жёстких» отказов подряд (ответ больше предела) → пропуск; «мягкий» отказ
//!   («узел ещё не дошёл») счётчик обнуляет; настройка 0 → пропуска по блоку нет;
//! - по длительности: отставание больше max_lag держится дольше bsc.receipts_stuck_reset_s → пропуск, причём таймер
//!   НЕ сбрасывается, когда лента ползёт вперёд (passed/failed по разным блокам) — ровно дыра живого бота, где прежний
//!   триггер по возрасту одного блока не срабатывал; реальный догон (≤ max_lag) таймер снимает.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, arb::venues::bsc_dex::node::{receipts_lag, receipts, events_lag}.

mod native_common;

use std::time::Duration;

use arb::settings::value::Scope;
use arb::venues::bsc_dex::node::events_lag::EventsLag;
use arb::venues::bsc_dex::node::receipts::Feed;
use arb::venues::bsc_dex::node::receipts_lag::Lag;
use serde_json::json;

const WHY: &str = "блок 129907931: квитанции больше предела bsc.receipts_total_ms (15000 мс), получено 1016252 байт";

#[test]
fn block_skip_after_hard_tries() {
    let mut lag = Lag::default();
    // Три «жёстких» отказа подряд (ответ больше предела) при K = 3 — пропуск.
    for _ in 0..3 {
        lag.failed(129907931, WHY, false, true);
    }
    assert!(
        lag.overdue_block(3).is_some(),
        "K жёстких отказов — пропуск блока"
    );
    assert!(
        lag.overdue_block(0).is_none(),
        "настройка 0 — пропуска по блоку нет"
    );
}

#[test]
fn soft_failure_resets_hard_count() {
    let mut lag = Lag::default();
    lag.failed(129907931, WHY, false, true);
    lag.failed(129907931, WHY, false, true);
    // «Мягкий» отказ (узел ещё не дошёл) — счёт жёстких обнуляется.
    lag.failed(129907931, "узел ещё не отдал блок 129907931", false, false);
    assert!(
        lag.overdue_block(3).is_none(),
        "после мягкого отказа жёстких < K"
    );
    lag.failed(129907931, WHY, false, true);
    assert!(lag.overdue_block(1).is_some());
}

#[tokio::test]
async fn duration_trigger_survives_crawling() {
    let ctx = native_common::ctx("receipts-resilience").await;
    ctx.settings
        .set(
            "bsc.receipts_stuck_reset_s",
            Scope::Global,
            Some(json!(1)),
            "тест",
        )
        .await
        .unwrap();
    let flag = EventsLag::default();
    let feed = Feed::with_lag(flag.clone());
    let mut lag = Lag::default();
    let mut told = false;

    // Отстали больше предела — таймер непрерывного отставания пошёл.
    lag.mark(&ctx, &flag, 70, 20).await;
    assert!(flag.busy(), "котировки запрещены, пока отстаём");
    // Пока таймер не достиг порога — пропуска нет.
    assert!(lag.overdue_behind(&ctx, &feed, &mut told).await.is_none());

    // Лента ползёт: разбирает блоки и спотыкается на следующих — застрявший блок всё время разный. Прежний триггер по
    // возрасту ОДНОГО блока тут сбрасывался бы каждый раз; таймер отставания — нет.
    for n in 0..5u64 {
        lag.passed(129907931 + n);
        lag.failed(129907931 + n + 1, WHY, false, true);
        lag.mark(&ctx, &flag, 70 + n, 20).await;
    }
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(
        lag.overdue_behind(&ctx, &feed, &mut told).await.is_some(),
        "отставание держится дольше bsc.receipts_stuck_reset_s несмотря на ползущую ленту — пропуск"
    );

    // Реальный догон (≤ max_lag) снимает таймер и запрет.
    lag.mark(&ctx, &flag, 5, 20).await;
    assert!(!flag.busy(), "догнали — котировки разрешены");
    assert!(
        lag.overdue_behind(&ctx, &feed, &mut told).await.is_none(),
        "после догона таймера отставания нет"
    );
}
