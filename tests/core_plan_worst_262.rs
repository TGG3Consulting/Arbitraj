//! Файл: tests/core_plan_worst_262.rs
//! Что делает: решение 262 — худший уровень покупки плана (Calc::buy_worst, лимит ордера покупки на бирже) в настоящем
//! расчёте ядра costs::evaluate_with на записанных данных CAKE (tests/report_plan: стакан BingX CAKE-USDT 23.10, пул
//! CAKE/USDT V2): равен самому глубокому уровню записанных asks, нужному на объём плана (gateway/frozen.rs deepest —
//! та же функция, что у ядра), не ниже средней цены покупки; spend_max посчитан по нему.
//! Ордер с этим лимитом — tests/core_plan_limit_262.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::costs, arb::gateway::frozen, tests/report_plan, tests/size_common.

mod report_plan;
mod size_common;

use arb::core::types::{OrderBook, Side, VenueId};

#[tokio::test]
async fn plan_buy_worst_is_deepest_level_of_plan_book() {
    let plan = report_plan::plan().await;
    let rec = size_common::Rec::load("res-262").await;
    let book = OrderBook {
        venue: VenueId("bingx".into()),
        asset: plan.direction.asset.clone(),
        bids: vec![],
        asks: rec.asks.clone(),
        exchange_time: None,
        received_at: chrono::Utc::now(),
    };
    let deepest = arb::gateway::frozen::deepest(&book, Side::Buy, plan.qty).unwrap();
    assert_eq!(plan.buy_worst, deepest, "худший уровень покупки плана");
    assert!(plan.buy_worst >= plan.buy_quote.avg_price);
    assert_eq!(plan.spend_max, plan.qty * plan.buy_worst + plan.spend_fee);
}
