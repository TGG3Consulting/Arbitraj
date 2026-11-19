//! Файл: tests/fit_common/mod.rs
//! Что делает: общее для тестов решения 294 на записи цепи tests/data/fit_volume_2026-11-15 (пул PancakeSwap V3
//! SHOPINX/WBNB и пара PancakeSwap V2 WBNB/USDT, блок 133732042): сцена (настоящая SQLite, пулы записи, карта ±2 слова —
//! dex.tick_words = 2 на сервере 15.11, нулевые налоги, допуск пулов, прямой режим, классика), котировка выбора
//! маршрута (quote/routing.rs run), включатель dex.intermediate_partial_fill, котировка в JSON для сравнения «1 в 1».
//! Кто вызывает: tests/core_fit_volume_294.rs, tests/bsc_dex_fit_graph_294.rs (каждому нужна своя часть —
//! allow(dead_code)); подключать после `mod route_common;`.
//! От чего зависит: tests/route_common (load, v3, v2, ctx, clean_taxes), arb::venues::bsc_dex::{parts, quote}.
#![allow(dead_code)]

use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{Asset, CheckState, CoinCheck, Quote, Side, TokenTaxes, VenueId};
use arb::gateway::errors::GatewayResult;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::mids::Mid;
use arb::venues::bsc_dex::quote::{Quoting, routing};

pub const DIR: &str = "tests/data/fit_volume_2026-11-15";
pub const BLOCK: u64 = 133_732_042;
pub const GAS: u64 = 1_000_000_000;
/// Адреса — из записи (token0/token1 пула и пары).
pub const SHOPINX: Address = address!("0x4f0ed1f48beaa4797a62202a21804cadc2170d6b");
pub const WBNB: Address = address!("0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c");
pub const USDT: Address = address!("0x55d398326f99059ff775485246999027b3197955");
pub const POOL: Address = address!("0x3079ae2e87dc2e0c75c9b98d65bc622ad10f326c");
pub const PAIR: Address = address!("0x16b9a82891338f9ba80e2d6970fdda79d1eb0dae");
/// Текущий тик записи −111229 → слово битмапа −435 (tickSpacing 1).
pub const WORD: i16 = -435;

pub fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

pub fn shopinx() -> Asset {
    Asset {
        chain: None,
        ticker: "SHOPINX".into(),
        contract: Some(SHOPINX),
    }
}

pub fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

/// Запись: пул SHOPINX/WBNB (слова ±3) и пара WBNB/USDT, WBNB — промежуточная, налоги SHOPINX нулевые, пулы допущены
/// (газ и сверка с квотером — как route_common), свежая голова, прямой режим, классика. Ошибок нет.
pub async fn scene() -> Parts {
    let got = super::route_common::load(DIR);
    let p = Parts::new(super::route_common::ctx().await);
    let usdt = Asset {
        chain: None,
        ticker: "USDT".into(),
        contract: Some(USDT),
    };
    p.tokens.set(&[usdt, shopinx()]);
    for t in [SHOPINX, WBNB, USDT] {
        p.tokens.remember_decimals(t, 18);
    }
    // Налоги SHOPINX и WBNB (промежуточная, Via::mid_ok) — проверены и нулевые, как CAKE в route_common.
    let wbnb = Asset {
        chain: None,
        ticker: "BNB".into(),
        contract: Some(WBNB),
    };
    for asset in [shopinx(), wbnb] {
        p.checks.set_coin(CoinCheck {
            venue: VenueId("bsc_dex".into()),
            asset,
            taxes: CheckState::Known(zero()),
            source: "тест".into(),
            checked_at: Some(Utc::now()),
            wait: None,
        });
    }
    p.mids.set(vec![Mid {
        address: WBNB,
        ticker: "BNB".into(),
    }]);
    let tag = "SHOPINX-WBNB pcs_v3";
    // dex.tick_words = 2 — значение на сервере 15.11 (факт настроек бота); запись — ±12 слов.
    let words = (WORD - 2, WORD + 2);
    let mut pool = super::route_common::v3(&got, tag, true, 1, 100, words, POOL, SHOPINX, WBNB);
    pool.ticker = "SHOPINX".into();
    let pair = super::route_common::v2(&got, "WBNB-USDT v2", PAIR, WBNB, USDT);
    for e in [pool, pair] {
        let r = e.pool.reference.clone();
        p.registry.put(e);
        p.checks.update_pool(&r, |f| {
            f.gas_buy = Some(120_000);
            f.gas_sell = Some(120_000);
            f.quoter.buy = Some(Ok(()));
            f.quoter.sell = Some(Ok(()));
            f.taxes = Some(super::route_common::clean_taxes());
        });
    }
    let v = Scope::Venue("bsc_dex".into());
    for (n, val) in [
        ("feed.stale_after_ms", json!(600_000)),
        ("dex.swap_mode", json!("direct")),
        ("dex.router_mode", json!("classic")),
    ] {
        p.ctx
            .settings
            .set(n, v.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    p.heads.observe(BLOCK);
    p
}

/// Котировка выбора маршрута адаптера (quote/routing.rs run). Принимает: котировку, сторону, объём SHOPINX.
pub async fn full(q: &Quoting, side: Side, qty: Decimal) -> GatewayResult<Quote> {
    let mids = q
        .mids
        .active(&q.ctx)
        .into_iter()
        .map(|m| (m, 18u8))
        .collect();
    let input = routing::Input {
        asset: shopinx(),
        side,
        qty,
        coin: SHOPINX,
        usdt: USDT,
        taxes: zero(),
        coin_dec: 18,
        usdt_dec: 18,
        entries: q.reg.by_pair(SHOPINX, USDT),
        mids,
        gas: U256::from(GAS),
        usdt_ticker: "USDT".into(),
        smart: false,
    };
    routing::run(q.clone(), input).await
}

/// Включатель решения 294 на площадке bsc_dex (None — снять). Ошибок нет.
pub async fn partial(p: &Parts, on: Option<bool>) {
    let v = Scope::Venue("bsc_dex".into());
    let val = on.map(|b| json!(b));
    p.ctx
        .settings
        .set("dex.intermediate_partial_fill", v, val, "тест")
        .await
        .unwrap();
}

/// Котировка в JSON (сравнение «1 в 1»). Ошибок нет.
pub fn js(q: &GatewayResult<Quote>) -> serde_json::Value {
    match q {
        Ok(q) => {
            // Время котировки — не часть расчёта.
            let mut v = serde_json::to_value(q).unwrap();
            v.as_object_mut().unwrap().remove("at");
            v
        }
        Err(e) => json!({"code": format!("{:?}", e.code), "message": e.message}),
    }
}
