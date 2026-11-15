//! Файл: tests/core_fit_volume_294.rs
//! Что проверяет: решение 294 — объём, не помещающийся в путь DEX через промежуточную монету, уменьшается до
//! помещающегося (потолок — предел, а не обязательный объём), на РЕАЛЬНОЙ записи цепи 15.11.2026
//! (tests/data/fit_volume_2026-11-15, блок 133732042): пул PancakeSwap V3 SHOPINX/WBNB (fee 100, tickSpacing 1) и пара
//! PancakeSwap V2 WBNB/USDT. Пула SHOPINX с USDT нет — котировка идёт только через WBNB (quote/via_pools.rs). Карта
//! тиков — слова битмапа ±3 вокруг текущего (как при dex.tick_words = 3; записано ±12, все слова пусты — у пула только
//! позиции на краях диапазона, факт README записи), за её край шаг пути «не исполним целиком» → NoRoute
//! (quote/route_calc.rs). Котировка — выбор маршрута адаптера целиком (quote/routing.rs run, как Quoting::priced до
//! газа разрешений и чаевых; цену газа узла тест не спрашивает — 1 gwei параметром, как tests/bsc_dex_quote_fills.rs).
//! (1) Сигнал (core/fit_volume.rs halve, тот же код, что зовёт prices.rs): объём проверки 57000 SHOPINX (верх живого
//!     диапазона 33–57 тыс.) не помещается — код NoRoute распознаётся тем же правилом, что подбор объёма
//!     (costs_quote.rs classify); signal.fit_halvings не задана — tries = None (повторов нет, prices.rs отдаёт прежнюю
//!     ошибку); задана — котировка на половине, она полная (fillable = объём); минимум площадок выше половины — отказ.
//! (2) Финальный расчёт (core/final_fit.rs largest, тот же код, что зовёт profit.rs): объём 57000 не помещается —
//!     size.final_fit_steps не задана — tries = None (отказ, как раньше); задана — наибольший помещающийся объём:
//!     котировка полная, на шаг больше — снова NoRoute; с бюджетом 100 USDT — списание ≤ бюджета, на шаг больше — больше
//!     бюджета; один шаг бисекции — половина (безопасная сторона); шаг количества дороже бюджета — отказ.
//!     Пороги прибыли здесь не проверяются: записей стакана, правил пары и сети биржи по SHOPINX нет (полного расчёта
//!     costs.rs не собрать); пороги в profit.rs идут после final_calc прежним кодом. Точка кривой теста — котировка
//!     покупки DEX: вложение = средняя цена × объём (USDT котировки); прибыль поиском границы не используется
//!     (size_bounds.rs upper смотрит только на вид отказа и бюджет) — в точке 0. Шаг количества — 1 SHOPINX (пары
//!     биржи нет).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{fit_volume, final_fit, costs_quote, size}, arb::venues::bsc_dex::quote::routing,
//! tests/route_common (load, v3, v2, ctx).

mod route_common;

use alloy_primitives::{Address, U256, address};
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::costs_quote::classify;
use arb::core::final_fit::largest;
use arb::core::fit_volume::{QuoteErr, halve, tries};
use arb::core::size::{Curve, Fail, Point};
use arb::core::types::{Asset, CheckState, CoinCheck, Quote, Side, TokenTaxes, VenueId};
use arb::gateway::errors::GatewayResult;
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
    let words = (WORD - 3, WORD + 3);
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

#[tokio::test]
async fn signal_halves_volume_until_it_fits() {
    let p = scene().await;
    let q = p.quoting();
    let s = &p.ctx.settings;
    let want = d("57000");
    for side in [Side::Sell, Side::Buy] {
        let first = quote_err(&q, side, want).await.unwrap_err();
        assert!(first.no_route, "{side:?}: ожидался NoRoute: {}", first.text);
        assert!(first.text.contains("не исполним целиком"), "{}", first.text);
        // Без настройки повторов нет: prices.rs отдаёт эту ошибку как есть (fit_volume.rs retry).
        assert_eq!(tries(s, "signal.fit_halvings").unwrap(), None);
        // С настройкой — половина помещается, котировка полная.
        let (got, qty) = halve(want, Decimal::ZERO, 3, first.clone(), |x| {
            quote_err(&q, side, x)
        })
        .await
        .unwrap();
        assert_eq!(qty, d("28500"));
        assert_eq!(
            got.fillable_qty, qty,
            "{side:?}: котировка на {qty} неполная"
        );
        println!(
            "{side:?}: 57000 — NoRoute; 28500 — полная, средняя {}",
            got.avg_price
        );
        // Минимум площадок выше половины — повторов нет, отказ с исходной причиной.
        let e = halve(want, d("30000"), 3, first.clone(), |x| {
            quote_err(&q, side, x)
        })
        .await
        .unwrap_err();
        assert!(e.text.starts_with(&first.text) && e.text.contains("ниже минимума"));
    }
    s.set("signal.fit_halvings", Scope::Global, Some(json!(3)), "тест")
        .await
        .unwrap();
    assert_eq!(tries(s, "signal.fit_halvings").unwrap(), Some(3));
}

/// Кривая теста: точка — котировка покупки на DEX (вложение — USDT котировки).
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
    // Без настройки — отказ, как раньше (profit.rs final_calc).
    assert_eq!(tries(s, "size.final_fit_steps").unwrap(), None);
    // С настройкой: 20 шагов бисекции хватает до шага (log2 57000 < 16).
    let mut c = DexBuy { q: &q };
    let big = d("1000000");
    let pt = largest(&mut c, qty, over.clone(), big, step, 20)
        .await
        .unwrap();
    assert!(pt.qty < qty && pt.qty > Decimal::ZERO);
    assert_eq!(
        pt.data.fillable_qty, pt.qty,
        "котировка на {} неполная",
        pt.qty
    );
    let next = full(&q, Side::Buy, pt.qty + step).await.unwrap_err();
    assert_eq!(
        classify("bsc_dex", &next),
        Fail::TooLarge(format!("bsc_dex: {}", next.message))
    );
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
    // Шагов мало — безопасная сторона (помещающийся, не наибольший): один шаг бисекции — половина.
    let p1 = largest(&mut c, qty, over.clone(), big, step, 1)
        .await
        .unwrap();
    assert_eq!(p1.qty, d("28500"));
    // Даже один шаг количества дороже бюджета — помещающегося нет, отказ.
    assert!(
        largest(&mut c, qty, over, d("0.001"), step, 20)
            .await
            .is_err()
    );
    s.set(
        "size.final_fit_steps",
        Scope::Global,
        Some(json!(20)),
        "тест",
    )
    .await
    .unwrap();
    assert_eq!(tries(s, "size.final_fit_steps").unwrap(), Some(20));
}
