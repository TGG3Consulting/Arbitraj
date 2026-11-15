//! Файл: tests/core_fit_volume_294.rs
//! Что проверяет: решение 294 — объём, не помещающийся в путь DEX через промежуточную монету, уменьшается до
//! помещающегося (потолок — предел, а не обязательный объём), на РЕАЛЬНОЙ записи цепи 15.11.2026
//! (tests/data/fit_volume_2026-11-15, блок 133732042): пул PancakeSwap V3 SHOPINX/WBNB (fee 100, tickSpacing 1) и пара
//! PancakeSwap V2 WBNB/USDT. Пула SHOPINX с USDT нет — котировка идёт только через WBNB (quote/via_pools.rs).
//! Записано с цепи: состояние пулов и слова битмапа ±12. Задано разработчиком (см. README записи): карта ±2 слова
//! (dex.tick_words = 2 — значение на сервере 15.11), объём проверки 57000 SHOPINX, бюджет 100 USDT, шаг 1 SHOPINX,
//! цена газа 1 gwei, нулевые налоги SHOPINX и WBNB, допуск пулов. За край карты шаг пути «не исполним целиком» →
//! NoRoute с признаком too_large (quote/route_calc.rs). Котировка — выбор маршрута адаптера целиком (quote/routing.rs
//! run, как Quoting::priced до газа разрешений и чаевых; цену газа узла тест не спрашивает).
//! (1) Сигнал (core/fit_volume.rs halve — тот же код, что зовёт prices.rs): не помещается — признак too_large; без
//!     signal.fit_halvings — None (повторов нет); с ней — делим пополам до помещающегося (котировка полная, объём не
//!     больше вместимости, вдвое больший — больше); минимум площадок: половина ниже минимума — одна попытка на минимуме
//!     (помещается — котировка на нём; нет — одна котировка и отказ); минимум не меньше объёма — котировок нет.
//! (2) Постоянный NoRoute (промежуточные выключены) — признака нет, ни одной лишней котировки.
//! (3) Финальный расчёт (core/final_fit.rs largest — тот же код, что зовёт final_fill.rs): наибольший помещающийся
//!     объём = вместимость (бисекция по настоящим котировкам), на шаг больше — снова «не помещается»; бюджет 100 USDT —
//!     списание ≤ бюджета; шаг количества дороже бюджета — отказ; значение size.final_fit_steps больше u32 — неверное.
//!     Пороги прибыли и путь через prices.rs / profit.rs (FinalCurve, costs::evaluate) здесь НЕ проверяются: записей
//!     стакана, правил пары и сети биржи по SHOPINX нет. Точка кривой — котировка покупки DEX (вложение = средняя
//!     цена × объём); прибыли нет — поиск границы (size_bounds.rs upper) смотрит только на вид отказа и бюджет.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{fit_volume, fit_settings, final_fit, costs_quote, size},
//! arb::venues::bsc_dex::quote::routing, tests/route_common (load, v3, v2, ctx).

mod route_common;

use std::sync::atomic::{AtomicU32, Ordering};

use alloy_primitives::{Address, U256, address};
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::costs_quote::classify;
use arb::core::final_fit::largest;
use arb::core::fit_settings::read;
use arb::core::fit_volume::{QuoteErr, halve};
use arb::core::size::{Curve, Fail, Point};
use arb::core::types::{Asset, CheckState, CoinCheck, Quote, Side, TokenTaxes, VenueId};
use arb::gateway::errors::{ErrorCode, GatewayResult};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::mids::Mid;
use arb::venues::bsc_dex::quote::{Quoting, routing};

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

/// Котировка с ошибкой в виде ядра (QuoteErr — то же правило, что у prices.rs).
async fn quote_err(q: &Quoting, side: Side, qty: Decimal) -> Result<Quote, QuoteErr> {
    full(q, side, qty)
        .await
        .map_err(|e| QuoteErr::of("bsc_dex", &e))
}

/// Наибольший объём покупки или продажи (шаг 1 SHOPINX), чья котировка полная: бисекция по настоящим котировкам.
async fn capacity(q: &Quoting, side: Side, over: Decimal) -> Decimal {
    let (mut lo, mut hi) = (Decimal::ZERO, over);
    while hi - lo > Decimal::ONE {
        let mid = ((lo + hi) / Decimal::TWO).floor();
        match quote_err(q, side, mid).await {
            Ok(x) if x.fillable_qty == mid => lo = mid,
            _ => hi = mid,
        }
    }
    lo
}

#[tokio::test]
async fn signal_halves_volume_until_it_fits() {
    let p = scene().await;
    let q = p.quoting();
    let s = &p.ctx.settings;
    let want = d("57000");
    // Без настройки повторов нет: prices.rs отдаёт первую ошибку как есть (fit_volume.rs retry).
    assert_eq!(read(s, "signal.fit_halvings").unwrap(), None);
    for side in [Side::Sell, Side::Buy] {
        let first = quote_err(&q, side, want).await.unwrap_err();
        assert!(
            first.too_large,
            "{side:?}: ожидался «не помещается»: {}",
            first.text
        );
        assert!(first.text.contains("не исполним целиком"), "{}", first.text);
        let cap = capacity(&q, side, want).await;
        // С настройкой — делим пополам, пока не поместится; котировка полная, объём не больше вместимости.
        let calls = AtomicU32::new(0);
        let quote = |x: Decimal| {
            calls.fetch_add(1, Ordering::Relaxed);
            quote_err(&q, side, x)
        };
        let (got, qty) = halve(want, Decimal::ZERO, 4, first.clone(), quote)
            .await
            .unwrap();
        assert_eq!(
            got.fillable_qty, qty,
            "{side:?}: котировка на {qty} неполная"
        );
        assert!(
            qty <= cap && qty * Decimal::TWO > cap,
            "{side:?}: {qty} при вместимости {cap}"
        );
        let n = calls.load(Ordering::Relaxed);
        println!(
            "{side:?}: 57000 — не помещается; вместимость {cap}; найдено {qty} за {n} котировок"
        );
        // Минимум площадок (решение 294, п. 3 ревью): половина ниже минимума, минимум меньше объёма — одна попытка
        // на минимуме. Объём проверки — 1,5 вместимости (половина ниже минимума в обоих случаях).
        let want2 = (cap * d("1.5")).floor();
        let first2 = quote_err(&q, side, want2).await.unwrap_err();
        assert!(first2.too_large);
        // Минимум помещается — котировка на минимуме.
        let (got, qty) = halve(want2, cap, 4, first2.clone(), |x| quote_err(&q, side, x))
            .await
            .unwrap();
        assert_eq!((qty, got.fillable_qty), (cap, cap));
        // Минимум не помещается — одна котировка на нём, потом отказ с исходной причиной.
        let calls = AtomicU32::new(0);
        let quote = |x: Decimal| {
            calls.fetch_add(1, Ordering::Relaxed);
            quote_err(&q, side, x)
        };
        let e = halve(want2, cap + Decimal::ONE, 4, first2.clone(), quote)
            .await
            .unwrap_err();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(
            e.text.starts_with(&first2.text) && e.text.contains("пробовали до"),
            "{}",
            e.text
        );
        // Минимум не меньше объёма — котировок нет.
        let e = halve(want2, want2, 4, first2.clone(), |x| quote_err(&q, side, x))
            .await
            .unwrap_err();
        assert!(e.text.contains("меньше объёма нельзя"), "{}", e.text);
    }
    s.set("signal.fit_halvings", Scope::Global, Some(json!(3)), "тест")
        .await
        .unwrap();
    assert_eq!(read(s, "signal.fit_halvings").unwrap(), Some(3));
}

/// Постоянный NoRoute (промежуточные монеты выключены: «пулов с USDT нет; через промежуточную: …») признака «не
/// помещается» не несёт — повторов нет, ни одной лишней котировки (п. 2 ревью).
#[tokio::test]
async fn permanent_no_route_is_not_retried() {
    let p = scene().await;
    p.mids.set(vec![]);
    let q = p.quoting();
    for side in [Side::Sell, Side::Buy] {
        let e = full(&q, side, d("1000")).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::NoRoute, "{}", e.message);
        assert!(!e.too_large, "{}", e.message);
        let first = QuoteErr::of("bsc_dex", &e);
        let calls = AtomicU32::new(0);
        let quote = |x: Decimal| {
            calls.fetch_add(1, Ordering::Relaxed);
            quote_err(&q, side, x)
        };
        let r = halve(d("1000"), Decimal::ZERO, 4, first.clone(), quote).await;
        assert_eq!(r.unwrap_err().text, first.text);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }
}

/// Кривая теста поиска границы: точка — котировка покупки на DEX (вложение — USDT котировки). Прибыли у неё нет —
/// поиск границы (size_bounds.rs upper) смотрит только на вид отказа и бюджет; пороги прибыли здесь не проверяются.
struct DexBuy<'a> {
    q: &'a Quoting,
}

#[async_trait]
impl Curve for DexBuy<'_> {
    type Data = Quote;
    async fn at(&mut self, qty: Decimal) -> Result<Point<Quote>, Fail> {
        let r = full(self.q, Side::Buy, qty).await;
        let c = r.map_err(|e| classify("bsc_dex", &e))?;
        Ok(Point {
            qty,
            invested: c.avg_price * qty,
            arrive: qty,
            profit: Decimal::ZERO,
            data: c,
        })
    }
}

#[tokio::test]
async fn final_calc_shrinks_to_largest_fitting_volume() {
    let p = scene().await;
    let q = p.quoting();
    let s = &p.ctx.settings;
    let qty = d("57000");
    let step = Decimal::ONE;
    let over = classify("bsc_dex", &full(&q, Side::Buy, qty).await.unwrap_err());
    assert!(matches!(over, Fail::TooLarge(_)), "{over}");
    // Без настройки — отказ, как раньше (final_fill.rs).
    assert_eq!(read(s, "size.final_fit_steps").unwrap(), None);
    // С настройкой: 20 шагов бисекции хватает до шага (log2 57000 < 16) — найден наибольший помещающийся.
    let cap = capacity(&q, Side::Buy, qty).await;
    let mut c = DexBuy { q: &q };
    let big = d("1000000");
    let pt = largest(&mut c, qty, over.clone(), big, step, 20)
        .await
        .unwrap();
    assert_eq!(pt.qty, cap);
    assert_eq!(
        pt.data.fillable_qty, pt.qty,
        "котировка на {} неполная",
        pt.qty
    );
    let next = full(&q, Side::Buy, pt.qty + step).await.unwrap_err();
    assert!(next.too_large, "{}", next.message);
    println!(
        "помещается {} SHOPINX, вложение {} USDT",
        pt.qty,
        pt.invested.round_dp(4)
    );
    // Бюджет 100 USDT — списание не больше бюджета, на шаг больше — уже больше бюджета.
    let budget = d("100");
    let pb = largest(&mut c, qty, over.clone(), budget, step, 20)
        .await
        .unwrap();
    assert!(pb.invested <= budget && pb.qty < pt.qty);
    let up = c.at(pb.qty + step).await.unwrap();
    assert!(up.invested > budget, "{} → {}", pb.qty, up.invested);
    // Один шаг бисекции — середина (28500), помещающаяся сторона только если середина помещается.
    let p1 = largest(&mut c, qty, over.clone(), big, step, 1).await;
    match p1 {
        Ok(x) => assert!(x.qty == d("28500") && cap >= x.qty),
        Err(_) => assert!(cap < d("28500")),
    }
    // Даже один шаг количества дороже бюджета — помещающегося нет, отказ.
    assert!(
        largest(&mut c, qty, over, d("0.001"), step, 20)
            .await
            .is_err()
    );
    // Значение настройки: задано — число; больше u32 — неверное (уменьшения нет, запись Warn — fit_settings.rs tries).
    let set = |v: serde_json::Value| s.set("size.final_fit_steps", Scope::Global, Some(v), "тест");
    set(json!(20)).await.unwrap();
    assert_eq!(read(s, "size.final_fit_steps").unwrap(), Some(20));
    set(json!(5_000_000_000u64)).await.unwrap();
    assert!(read(s, "size.final_fit_steps").is_err());
}
