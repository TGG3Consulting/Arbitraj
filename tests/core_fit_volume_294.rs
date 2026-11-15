//! Файл: tests/core_fit_volume_294.rs
//! Что проверяет: решение 294 (прямой способ) — путь через промежуточную монету, которому не хватает прочитанной карты
//! тиков, при dex.intermediate_partial_fill = да отдаёт помещающуюся часть (fillable_qty < qty) одним расчётом по
//! пулу (quote/route_fit.rs, вызов — quote/via_pools.rs), на РЕАЛЬНОЙ записи цепи 15.11.2026
//! (tests/data/fit_volume_2026-11-15, блок 133732042): пул PancakeSwap V3 SHOPINX/WBNB (fee 100, tickSpacing 1) и пара
//! PancakeSwap V2 WBNB/USDT. Пула SHOPINX с USDT нет — котировка идёт только через WBNB.
//! Записано с цепи: состояние пулов и слова битмапа ±12. Задано разработчиком (README записи): карта ±2 слова
//! (dex.tick_words = 2 — значение на сервере 15.11), объём 57000 SHOPINX (верх живого диапазона 33–57 тыс.), цена газа
//! 1 gwei, нулевые налоги SHOPINX и WBNB, допуск пулов. Котировка — выбор маршрута адаптера целиком (quote/routing.rs
//! run, как Quoting::priced до газа разрешений и чаевых; цену газа узла тест не спрашивает).
//! Время котировки (поле at) при сравнении «1 в 1» не учитывается.
//! Проверяется: (1) настройка не задана / «нет» — прежний NoRoute, тот же текст; (2) да — fillable = помещающийся
//! объём: сверка тем же кодом без настройки — на нём котировка полная, на 1 наименьшую единицу SHOPINX больше —
//! прежний NoRoute; продажа (предел на первом шаге) и покупка (предел на последнем шаге); (3) объём помещается —
//! котировка 1 в 1 с настройкой и без; (4) режим graph — с настройкой и без одинаково (там часть отдаёт граф, 271);
//! (5) V2 «выход ≥ резерва» (записанная пара WBNB/USDT) — помещающейся части нет (прежний NoRoute).
//! Нет записи: предел на втором шаге продажи / первом шаге покупки (нужен путь с узким вторым пулом), налог монеты ≠ 0
//! — docs/TASKS.md.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{routing, route_fit, route_calc}, tests/route_common (load, v3, v2, ctx).

mod route_common;

use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{Asset, CheckState, CoinCheck, Quote, Side, TokenTaxes, VenueId};
use arb::gateway::errors::{ErrorCode, GatewayResult};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::mids::Mid;
use arb::venues::bsc_dex::quote::route_calc::{self, Leg};
use arb::venues::bsc_dex::quote::select::Usable;
use arb::venues::bsc_dex::quote::{Quoting, route_fit, routing};

const DIR: &str = "tests/data/fit_volume_2026-11-15";
const BLOCK: u64 = 133_732_042;
const GAS: u64 = 1_000_000_000;
/// Адреса — из записи (token0/token1 пула и пары).
const SHOPINX: Address = address!("0x4f0ed1f48beaa4797a62202a21804cadc2170d6b");
const WBNB: Address = address!("0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c");
const USDT: Address = address!("0x55d398326f99059ff775485246999027b3197955");
const POOL: Address = address!("0x3079ae2e87dc2e0c75c9b98d65bc622ad10f326c");
const PAIR: Address = address!("0x16b9a82891338f9ba80e2d6970fdda79d1eb0dae");
/// Текущий тик записи −111229 → слово битмапа −435 (tickSpacing 1).
const WORD: i16 = -435;

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

fn shopinx() -> Asset {
    Asset {
        ticker: "SHOPINX".into(),
        bsc_contract: Some(SHOPINX),
    }
}

fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

/// Запись: пул SHOPINX/WBNB (слова ±3) и пара WBNB/USDT, WBNB — промежуточная, налоги SHOPINX нулевые, пулы допущены
/// (газ и сверка с квотером — как route_common), свежая голова, прямой режим, классика. Ошибок нет.
async fn scene() -> Parts {
    let got = route_common::load(DIR);
    let p = Parts::new(route_common::ctx().await);
    let usdt = Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(USDT),
    };
    p.tokens.set(&[usdt, shopinx()]);
    for t in [SHOPINX, WBNB, USDT] {
        p.tokens.remember_decimals(t, 18);
    }
    // Налоги SHOPINX и WBNB (промежуточная, Via::mid_ok) — проверены и нулевые, как CAKE в route_common.
    let wbnb = Asset {
        ticker: "BNB".into(),
        bsc_contract: Some(WBNB),
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
    let mut pool = route_common::v3(&got, tag, true, 1, 100, words, POOL, SHOPINX, WBNB);
    pool.ticker = "SHOPINX".into();
    let pair = route_common::v2(&got, "WBNB-USDT v2", PAIR, WBNB, USDT);
    for e in [pool, pair] {
        let r = e.pool.reference.clone();
        p.registry.put(e);
        p.checks.update_pool(&r, |f| {
            f.gas_buy = Some(120_000);
            f.gas_sell = Some(120_000);
            f.quoter.buy = Some(Ok(()));
            f.quoter.sell = Some(Ok(()));
            f.taxes = Some(route_common::clean_taxes());
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
async fn full(q: &Quoting, side: Side, qty: Decimal) -> GatewayResult<Quote> {
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
async fn partial(p: &Parts, on: Option<bool>) {
    let v = Scope::Venue("bsc_dex".into());
    let val = on.map(|b| json!(b));
    p.ctx
        .settings
        .set("dex.intermediate_partial_fill", v, val, "тест")
        .await
        .unwrap();
}

/// Котировка в JSON (сравнение «1 в 1»). Ошибок нет.
fn js(q: &GatewayResult<Quote>) -> serde_json::Value {
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

#[tokio::test]
async fn partial_fill_gives_exact_fitting_volume() {
    let p = scene().await;
    let q = p.quoting();
    let want = d("57000");
    let unit = Decimal::new(1, 18);
    for side in [Side::Sell, Side::Buy] {
        // (1) не задана — прежний NoRoute; «нет» — тот же текст.
        partial(&p, None).await;
        let off = full(&q, side, want).await.unwrap_err();
        assert_eq!(off.code, ErrorCode::NoRoute);
        assert!(
            off.message.contains("не исполним целиком"),
            "{}",
            off.message
        );
        partial(&p, Some(false)).await;
        let no = full(&q, side, want).await.unwrap_err();
        assert_eq!((no.code, &no.message), (off.code, &off.message));
        // (2) да — помещающаяся часть.
        partial(&p, Some(true)).await;
        let got = full(&q, side, want).await.unwrap();
        let fit = got.fillable_qty;
        assert!(fit > Decimal::ZERO && fit < want, "{side:?}: {fit}");
        println!(
            "{side:?}: 57000 не помещается — помещается {fit}, средняя {}",
            got.avg_price
        );
        // Сверка тем же кодом без настройки: на помещающемся объёме полная, на единицу больше — прежний NoRoute.
        partial(&p, None).await;
        let at = full(&q, side, fit).await.unwrap();
        assert_eq!(at.fillable_qty, fit);
        // Часть = полная котировка на этом объёме; отличается только запрошенный объём (qty — 57000).
        assert_eq!(got.qty, want);
        let mut part = got.clone();
        part.qty = fit;
        assert_eq!(
            js(&Ok(at)),
            js(&Ok(part)),
            "{side:?}: часть = полная котировка на ней"
        );
        let over = full(&q, side, fit + unit).await.unwrap_err();
        assert_eq!(over.code, ErrorCode::NoRoute, "{}", over.message);
        // (3) помещается — 1 в 1 с настройкой и без.
        let small = d("1000");
        let a = full(&q, side, small).await;
        partial(&p, Some(true)).await;
        let b = full(&q, side, small).await;
        assert_eq!(js(&a), js(&b));
        assert_eq!(a.unwrap().fillable_qty, small);
    }
}

#[tokio::test]
async fn graph_mode_is_unchanged() {
    let p = scene().await;
    let q = p.quoting();
    let v = Scope::Venue("bsc_dex".into());
    p.ctx
        .settings
        .set("dex.router_mode", v, Some(json!("graph")), "тест")
        .await
        .unwrap();
    for side in [Side::Sell, Side::Buy] {
        partial(&p, None).await;
        let a = full(&q, side, d("57000")).await;
        partial(&p, Some(true)).await;
        let b = full(&q, side, d("57000")).await;
        assert_eq!(js(&a), js(&b), "{side:?}");
    }
}

/// V2, покупка, выход ≥ резерва (math/v2.rs amount_in): частичного итога у V2 нет — помещающейся части нет, прежний
/// NoRoute (резерв − 1 помещается только при бесконечной цене — DECISIONS 294). Записанная пара WBNB/USDT: покупка
/// WBNB за USDT на весь резерв WBNB.
#[tokio::test]
async fn v2_reserve_limit_keeps_no_route() {
    let p = scene().await;
    let pair = p.registry.by_pair(WBNB, USDT).into_iter().next().unwrap();
    let reserve_wbnb = match pair.pool.state {
        arb::core::types::PoolState::V2 { reserve1, .. } => reserve1,
        _ => panic!("ожидалась пара V2"),
    };
    let qty = arb::venues::bsc_dex::num::to_decimal(reserve_wbnb, 18).unwrap();
    let usable = Usable {
        gas: 120_000,
        hook_pct: Decimal::ZERO,
        tax_pct: Decimal::ZERO,
    };
    let legs = vec![Leg {
        entry: pair,
        usable,
        token_in: USDT,
        token_out: WBNB,
        dec_in: 18,
        dec_out: 18,
    }];
    let e = route_calc::eval(&p.ctx, &legs, Side::Buy, qty, Decimal::ONE).unwrap_err();
    assert_eq!(e.code, ErrorCode::NoRoute, "{}", e.message);
    assert!(route_fit::fit(&p.ctx, &legs, Side::Buy, qty, Decimal::ONE).is_none());
}
