//! Файл: tests/report_plan/mod.rs
//! Что делает: решение 257 (приёмка t90, п. 6) — НАСТОЯЩИЙ план сделки для отчёта «Итоги сделок»: расчёт ядра
//! costs::evaluate_with (та же модель, что у бота) на записанных данных CAKE (tests/size_common: стакан BingX
//! CAKE-USDT, пул CAKE/USDT V2) источником пересчёта по окну (core/uncapped_replay.rs, как tests/core_price_reserve.rs)
//! с возвратом USDT «в каждой сделке» и σ > 0 — в плане есть строки запаса на движение цены (PriceMoveReserve) и
//! «возврат USDT». Строка возврата источника — записанная комиссия вывода USDT BEP20 у BingX 0.15
//! (tests/data/bingx_2026-10-24/config_getall.json, withdrawFee); σ — 0.00002 (как tests/core_price_reserve.rs).
//! Кто вызывает: tests/report_results.rs.
//! От чего зависит: arb::core::{costs, dirctx, registry, uncapped_replay}, tests/size_common.
#![allow(dead_code)]

use arb::core::costs::{self, Calc};
use arb::core::costs_types::RETURN_STAGE;
use arb::core::dirctx::DirCtx;
use arb::core::registry::Presence;
use arb::core::types::{Asset, Direction, OrderBook, VenueId};
use arb::core::uncapped_replay::{Plan, Replay};
use chrono::Utc;
use serde_json::json;

use crate::size_common::{CAKE, Rec, USDT, d};

fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    }
}

/// Направление bingx → bsc_dex по CAKE.
pub fn dir() -> Direction {
    Direction {
        asset: cake(),
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId("bsc_dex".into()),
    }
}

/// Контекст направления на записанных данных (k = 1, T = 120 с, возврат USDT в каждой сделке).
fn cx(rec: &Rec) -> DirCtx {
    let p = |id: &VenueId, dex: bool| Presence {
        venue: id.clone(),
        ticker: "CAKE".into(),
        network: (!dex).then(|| "BEP20".into()),
        can_receive: true,
        can_send: true,
        can_buy: true,
        can_sell: true,
        excluded: vec![],
        info: (!dex).then(|| rec.net.clone()),
        via: None,
    };
    let d_ = dir();
    DirCtx {
        a: p(&d_.buy_on, false),
        b: p(&d_.sell_on, true),
        dir: d_,
        net_a: "BEP20".into(),
        net_b: "BEP20".into(),
        quote: vec!["USDT".into(), "USDT".into()],
        usdt: Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
        },
        cex: vec![],
        sigma: d("0.00002"),
        transit_s: d("120"),
        k: d("1"),
        h: d("1"),
        closure_share: None,
        return_per_trade: true,
        pair_a: Some(rec.pair.clone()),
        pair_b: None,
        usdt_b: None,
        transfer_tax: Some(rust_decimal::Decimal::ZERO),
    }
}

/// Настоящий план (см. шапку). Возвращает: расчёт ядра на 10 CAKE.
pub async fn plan() -> Calc {
    let rec = Rec::load("res-plan").await;
    let mut pj = serde_json::to_value(crate::size_common::calc::calc(&rec, d("10"))).unwrap();
    pj["costs"].as_array_mut().unwrap().push(json!({
        "kind": "WithdrawFee", "stage": RETURN_STAGE, "amount": "0.15", "asset": "USDT", "usdt": "0.15",
        "in_price": false, "in_qty": false, "step": null
    }));
    let plan = Plan::parse(&pj).unwrap();
    let book = OrderBook {
        venue: VenueId("bingx".into()),
        asset: cake(),
        bids: rec.bids.clone(),
        asks: rec.asks.clone(),
        exchange_time: None,
        received_at: Utc::now(),
    };
    let src = Replay::new(&dir(), &plan, book, None, ["USDT", "USDT"]).unwrap();
    costs::evaluate_with(&cx(&rec), d("10"), &src)
        .await
        .unwrap()
}
