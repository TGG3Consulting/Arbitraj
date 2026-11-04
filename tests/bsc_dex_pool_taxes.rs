//! Файл: tests/bsc_dex_pool_taxes.rs
//! Что делает: налог монеты по пулу (решение 146) — чистые функции без сети:
//! 1) налог продажи по паре «ответ квотера / пришло» (check/pool_taxes.rs sell_pct, percent): ответ квотера 0 или
//!    продажа не измерена — НЕ ИЗМЕРЕН (None), не 0; пришло больше — 0; живой случай FLOKI 04.11 (6 продаж на DEX на
//!    0,297…0,309 % хуже плана при налоге монеты 0) — пара с удержанием 0,3 % даёт 0,3 %;
//! 2) налоги монеты для ядра по пулам (coin): покупка и продажа — наименьшие среди измеренных, перевод — наибольший;
//!    продажа нигде не измерена — налогов монеты нет; итог проверки (outcome): все продажи откатились без помех —
//!    «не прошла», помеха или квотер не ответил — «не проверено»;
//! 3) исключение пула в котировке (quote/pool_tax.rs usable через quote/select.rs): налог стороны не измерен или
//!    больше tax.max_sell_pct — пул не допущен с причиной; на записи пулов CAKE (tests/route_common, блок записи
//!    routes_2026-10-29) налог шага продажи прямого режима — налог ИМЕННО выбранного пула.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::check::{pool_taxes, probe, state, taxes}, quote::{direct, pool_tax, select},
//! tests/route_common (записанные пулы CAKE).

mod route_common;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use arb::core::types::{PoolRef, Side, TokenTaxes};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::pool_taxes;
use arb::venues::bsc_dex::check::probe::{Measured, SellMiss};
use arb::venues::bsc_dex::check::state::PoolTaxes;
use arb::venues::bsc_dex::check::taxes::Outcome;
use arb::venues::bsc_dex::quote::select;

/// 5·10^18 — сумма пробы записи tests/data/service_tax_2026-10-25 (налог там 0: дошло ровно столько же).
fn e18x5() -> U256 {
    U256::from(5u8) * U256::from(10u8).pow(U256::from(18u8))
}

/// Пришло на 0,3 % меньше ответа квотера (живой случай FLOKI 04.11). Ошибок нет.
fn floki_got(expected: U256) -> U256 {
    expected - expected * U256::from(3u8) / U256::from(1000u16)
}

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

fn pool(buy: &str, sell: Option<&str>, transfer: &str) -> PoolTaxes {
    PoolTaxes {
        buy_pct: d(buy),
        sell_pct: sell.map(d),
        transfer_pct: d(transfer),
    }
}

fn measured(sell: Result<(U256, U256), SellMiss>) -> Measured {
    Measured {
        buy_expected: e18x5(),
        buy_got: e18x5(),
        sell,
        transfer_sent: e18x5(),
        transfer_got: e18x5(),
        gas_buy: 0,
        gas_approve_buy: 0,
        gas_transfer: 0,
        gas_sell: None,
    }
}

#[test]
fn sell_tax_is_quoter_minus_got_and_never_silent_zero() {
    let x = e18x5();
    assert_eq!(pool_taxes::sell_pct(x, x, 18), Some(Decimal::ZERO));
    assert_eq!(pool_taxes::sell_pct(x, floki_got(x), 18), Some(d("0.3")));
    // Пришло больше расчёта — налога нет (0), а не отрицательный.
    assert_eq!(pool_taxes::sell_pct(x, x + x, 18), Some(Decimal::ZERO));
    // Ответ квотера 0 — делить не на что: не измерен.
    assert_eq!(pool_taxes::sell_pct(U256::ZERO, x, 18), None);
    // Продажа пробы откатилась — налог продажи пула не измерен, покупка и перевод — измерены.
    let miss = SellMiss {
        reverted: true,
        why: "пробная продажа откатилась".into(),
    };
    let t = pool_taxes::percent(&measured(Err(miss)), 18, 18).unwrap();
    assert_eq!(t, pool("0", None, "0"));
    let t = pool_taxes::percent(&measured(Ok((x, floki_got(x)))), 9, 18).unwrap();
    assert_eq!(t.sell_pct, Some(d("0.3")));
}

#[test]
fn coin_taxes_take_cleanest_pool_and_worst_transfer() {
    // FLOKI-подобная монета: V2 с налогом 0,3 % на обмен, чистый пул, пул без замера продажи.
    let pools = [
        pool("0.3", Some("0.3"), "0"),
        pool("0", Some("0"), "0.1"),
        pool("0.2", None, "0"),
    ];
    let t = pool_taxes::coin(&pools).unwrap();
    assert_eq!(
        t,
        TokenTaxes {
            buy_pct: Decimal::ZERO,
            sell_pct: Decimal::ZERO,
            transfer_pct: d("0.1"),
        }
    );
    // Продажа не измерена ни в одном пуле — налогов монеты нет (не 0).
    assert_eq!(pool_taxes::coin(&[pool("0.3", None, "0")]), None);
    assert_eq!(pool_taxes::coin(&[]), None);
}

#[test]
fn outcome_when_no_pool_measured_sell() {
    let only_buy = [pool("0", None, "0")];
    let reverted = [(true, "V2: пробная продажа откатилась".to_string())];
    let none = || (Vec::new(), Vec::new(), Vec::new());
    assert!(matches!(
        pool_taxes::outcome(&only_buy, false, &reverted, none()),
        Outcome::Failed(w) if w.contains("налог продажи не измерен")
    ));
    // Помеха на другом пуле — вердикта нет.
    let env = (Vec::new(), vec!["V3: узел 429".to_string()], Vec::new());
    assert!(matches!(
        pool_taxes::outcome(&only_buy, false, &reverted, env),
        Outcome::Wait(_)
    ));
    // Квотер продажи не ответил — помеха, не вердикт.
    let no_quote = [(false, "V2: квотер продажи не ответил".to_string())];
    assert!(matches!(
        pool_taxes::outcome(&only_buy, false, &no_quote, none()),
        Outcome::Wait(_)
    ));
    // Продажа измерена хоть в одном пуле — налоги известны.
    let both = [pool("0", None, "0"), pool("0.3", Some("0.3"), "0")];
    assert!(matches!(
        pool_taxes::outcome(&both, false, &reverted, none()),
        Outcome::Known(t) if t.sell_pct == d("0.3")
    ));
}

/// Пул без замера налога продажи или с налогом выше tax.max_sell_pct в котировку по продаже не идёт; налог шага прямого
/// режима — налог выбранного пула (записанные пулы CAKE, tests/route_common).
#[tokio::test]
async fn pool_tax_gates_and_prices_the_step() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let e = p
        .registry
        .get(&PoolRef::Contract(route_common::CAKE_USDT_PCS))
        .expect("пул записи");
    p.checks
        .update_pool(&e.pool.reference, |f| f.taxes = Some(pool("0", None, "0")));
    let why = select::usable(&q, &e, Side::Sell, 18).unwrap_err();
    assert!(why.contains("не измерен"), "{why}");
    assert!(select::usable(&q, &e, Side::Buy, 18).is_ok());
    p.checks.update_pool(&e.pool.reference, |f| {
        f.taxes = Some(pool("0", Some("0.3"), "0"))
    });
    let u = select::usable(&q, &e, Side::Sell, 18).expect("0,3 % ниже порога 5 %");
    assert_eq!(u.tax_pct, d("0.3"));
    // Налог продажи шага — налог пула: в пул доходит на 0,3 % меньше монет.
    let qty = Decimal::from(10u8);
    let (s, _, _) =
        arb::venues::bsc_dex::quote::direct::calc(&q.ctx, &e, &u, Side::Sell, qty, (18, 18))
            .expect("расчёт");
    let into_pool = arb::venues::bsc_dex::num::to_decimal(s.amount_in, 18).unwrap();
    assert_eq!(into_pool, d("9.97"));
    route_common::tax_limits(&p.ctx, "0.1").await;
    let why = select::usable(&q, &e, Side::Sell, 18).unwrap_err();
    assert!(why.contains("tax.max_sell_pct"), "{why}");
    // Порог не задан — пул не используется с причиной «не задана настройка» (как у ядра, core/blacklist.rs).
    p.ctx
        .settings
        .set("tax.max_sell_pct", Scope::Global, None, "тест")
        .await
        .unwrap();
    let why = select::usable(&q, &e, Side::Sell, 18).unwrap_err();
    assert!(why.contains("tax.max_sell_pct"), "{why}");
}

/// Решение 39 по налогу пула (решение 146): пул V3 с измеренным налогом 0 годен, с налогом > 0 — нет (только V2).
#[tokio::test]
async fn taxed_pool_must_be_v2() {
    use arb::venues::bsc_dex::quote::pool_tax;
    let p = route_common::parts().await;
    let q = p.quoting();
    let e = p
        .registry
        .get(&PoolRef::Contract(route_common::CAKE_USDT_PCS))
        .expect("пул записи (V3)");
    let u = select::usable(&q, &e, Side::Buy, 18).expect("налог 0");
    assert!(pool_tax::v2_only(&e, &u).is_ok());
    p.checks.update_pool(&e.pool.reference, |f| {
        f.taxes = Some(pool("0.3", Some("0"), "0"))
    });
    let u = select::usable(&q, &e, Side::Buy, 18).expect("0,3 % ниже порога");
    let why = pool_tax::v2_only(&e, &u).unwrap_err();
    assert!(why.contains("только V2"), "{why}");
}
