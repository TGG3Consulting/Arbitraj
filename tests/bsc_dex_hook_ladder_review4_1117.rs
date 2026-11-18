//! Файл: tests/bsc_dex_hook_ladder_review4_1117.rs
//! Что проверяет: правки ревью 4 решения «335 (лесенка влияния)». Точки цикла — запись узла 17.11
//! (tests/data/hook_pools_ticks_2026-11-17, блок 134000292; GAIB 0x5c1b, BTCMT 0x6216: покупка 1 / 20 / 100 USDT —
//! наш полный обмен по карте тиков против CLQuoter, формула check/hooks_probe.rs impact). Точки проб перед сделкой
//! между суммами записи — УСЛОВНЫЕ (квотера на промежуточных суммах в записи нет): их значение — значение записи на
//! ближайшей большей сумме; проверяются правила хранения, а не поведение крючка. Лесенка с убыванием — условная (в
//! записях её нет), это чистое правило.
//! - Н1: точек сделки в лесенке хранилища не больше предела на сторону; точки цикла не трогаются; самая новая точка
//!   остаётся; влияние на сумме любой вытесненной точки не ниже её значения (не лучше факта) — в сценарии
//!   n1_trade_points_bounded; с решения 406 (интерполяция между точками) это не общее правило: n1_evicted — на сумме
//!   вытесненной точки интерполяция между соседками (вопрос Тиграну — docs/TASKS.md t190); наибольшее не падает.
//! - Н2: лесенка с убыванием после точки сделки остаётся «с убыванием» (сырые точки); с решения 406 значение — накопленный
//!   максимум, ниже наименьшей точки — её значение; неубывающая (BTCMT) выравнивается, как раньше.
//! - Н5: отказ выше всех точек стороны и повтор той же точки сделки итог не меняют — не пишутся.
//! - Н9: перемер в режиме одного числа снимает «не прошла» и не делает из одного числа лесенку; с лесенкой — точка.
//! - Н3: сторона «не прошла» видна ядру (core/pool_hook.rs pool_sides_off); лесенка в JSON — список, вход точки сделки —
//!   поле coins (не hex).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/hook_ticks_common, arb::venues::bsc_dex::{check::{hook_fold, hook_trim, hooks_probe,
//! reprobe_rule}, quote::hook_curve}, arb::core::{blacklist, types}.

mod hook_ticks_common;

use std::str::FromStr;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use arb::core::blacklist::pool_sides_off;
use arb::core::types::{
    Asset, CheckState, HookGroup, HookImpact, HookStep, PoolCheck, PoolRef, Side, VenueId,
};
use arb::venues::bsc_dex::check::hook_fold::{on_failure, on_success};
use arb::venues::bsc_dex::check::hook_trim::is_trade;
use arb::venues::bsc_dex::check::hooks_probe::impact;
use arb::venues::bsc_dex::check::reprobe_rule::only_side;
use arb::venues::bsc_dex::quote::hook_curve::{at, merge, monotone, top_pct};
use hook_ticks_common::{load, map, ours, pools, quotes};

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).expect("число")
}

/// Лесенка покупки пула по записи (1 / 20 / 100 USDT). Ошибок нет.
fn buy(pre: &str) -> Vec<HookStep> {
    let all = load();
    let ps = pools(&all);
    let qs = quotes(&all, &ps);
    let (id, p) = ps
        .into_iter()
        .find(|(id, _)| id.starts_with(pre))
        .expect("пул");
    let m = map(&p);
    let mut out: Vec<HookStep> = qs
        .iter()
        .filter(|q| q.id == id && q.buy)
        .map(|q| HookStep {
            amount_in: q.amount,
            usd: Decimal::from(q.usd),
            pct: impact(ours(&p, &m, q.z, true, q.amount, p.lp).amount_out, q.remote)
                .expect("влияние"),
            coins: None,
            quoter_out: Some(q.remote),
        })
        .collect();
    out.sort_by_key(|s| s.amount_in);
    assert_eq!(out.len(), 3);
    out
}

/// Условная точка сделки на входе x (значение — запись на ближайшей большей сумме, см. шапку).
fn trade(cycle: &[HookStep], x: U256) -> HookStep {
    let up = cycle
        .iter()
        .find(|s| s.amount_in >= x)
        .expect("в пределах записи");
    HookStep {
        amount_in: x,
        usd: Decimal::ZERO,
        pct: up.pct,
        coins: Some(Decimal::ONE),
        quoter_out: None,
    }
}

fn only_buy(steps: Vec<HookStep>) -> HookImpact {
    HookImpact {
        buy_pct: top_pct(&steps).unwrap_or_default(),
        buy: steps.into(),
        ..Default::default()
    }
}

#[test]
fn n1_trade_points_bounded() {
    let cycle = buy("0x5c1b4d20");
    let mut stored = only_buy(cycle.clone());
    let (lo, hi) = (cycle[1].amount_in, cycle[2].amount_in);
    let cap = 2usize;
    let mut all = Vec::new();
    for k in 1..=12u64 {
        let x = lo + (hi - lo) * U256::from(k) / U256::from(13u64);
        let p = trade(&cycle, x);
        all.push(p.clone());
        if let Some(n) = on_success(&stored, &only_buy(vec![p]), Some(cap)) {
            stored = n;
        }
        let trades = stored.buy.iter().filter(|s| is_trade(s)).count();
        assert!(trades <= cap, "точек сделки {trades} > {cap}");
        assert_eq!(
            stored.buy.iter().filter(|s| !is_trade(s)).count(),
            3,
            "точки цикла на месте"
        );
        assert!(
            stored.buy.iter().any(|s| s.amount_in == x),
            "самая новая точка остаётся"
        );
    }
    for p in &all {
        let v = at(&stored.buy, p.amount_in).expect("в измеренном");
        assert!(
            v >= p.pct.max(Decimal::ZERO),
            "не лучше факта на {}",
            p.amount_in
        );
    }
    assert!(stored.buy_pct >= cycle[2].pct);
}

/// Независимое ревью t173 (Н-1): значение вытесненной точки сделки получает БОЛЬШАЯ соседка — малые суммы не
/// завышаются. GAIB, цикл 1 / 20 / 100 USDT (запись), предел 2, сделки 21, 60, 90 USDT (условные значения — запись на
/// 100 USDT): вытесняется 21 (зазор 1/21 до цикловой 20); влияние на 20 USDT и наименьшее — как в записи.
#[test]
fn n1_evicted_value_goes_up_not_down() {
    let cycle = buy("0x5c1b4d20");
    let mut stored = only_buy(cycle.clone());
    let at_usd = |u: u64| cycle[1].amount_in * U256::from(u) / U256::from(20u64);
    for u in [21u64, 60, 90] {
        let p = trade(&cycle, at_usd(u));
        if let Some(n) = on_success(&stored, &only_buy(vec![p]), Some(2)) {
            stored = n;
        }
    }
    assert!(
        !stored.buy.iter().any(|s| s.amount_in == at_usd(21)),
        "вытеснена 21"
    );
    assert_eq!(
        at(&stored.buy, cycle[1].amount_in),
        at(&cycle, cycle[1].amount_in),
        "влияние на 20 USDT не завышено"
    );
    let least = |v: &[HookStep]| v.iter().map(|s| s.pct).min();
    assert_eq!(
        least(&stored.buy),
        least(&cycle),
        "наименьшая точка не поднята"
    );
    // Решение 406 (t190): между точками — интерполяция. Значение вытесненной 21 получила большая соседка (60); на 21
    // USDT влияние — интерполяция между 20 (цикл) и 60, не ниже точки 20 и не выше значения 60 (вопрос Тиграну о
    // вытеснении при интерполяции — docs/TASKS.md t190).
    let sixty = stored
        .buy
        .iter()
        .find(|s| s.amount_in == at_usd(60))
        .expect("точка 60");
    assert!(
        sixty.pct >= cycle[2].pct,
        "значение вытесненной у большей соседки"
    );
    let v = at(&stored.buy, at_usd(21)).expect("измерено");
    assert!(
        v >= cycle[1].pct && v <= sixty.pct,
        "интерполяция на 21: {v}"
    );
}

#[test]
fn n2_decreasing_ladder_running_max() {
    // Условная лесенка с убыванием (условные числа; в записи 18.11 убывание есть — tests/bsc_dex_hook_curve_t190.rs).
    let s = |x: u64, p: &str, usd: &str| HookStep {
        amount_in: U256::from(x),
        usd: d(usd),
        pct: d(p),
        coins: None,
        quoter_out: None,
    };
    let down = vec![s(10, "0.5", "10"), s(100, "0.3", "100")];
    assert!(!monotone(&down));
    let m = merge(&down, s(50, "0.4", "0"));
    // Решение 406 (Тигран 18.11): значение точки — накопленный максимум по ней и меньшим суммам у любой лесенки; ниже
    // наименьшей — её значение (прежнее «не измерено» убрано).
    assert_eq!(
        at(&m, U256::from(5u64)),
        Some(d("0.5")),
        "ниже наименьшей — её значение"
    );
    assert_eq!(
        at(&m, U256::from(50u64)),
        Some(d("0.5")),
        "накопленный максимум"
    );
    assert_eq!(at(&m, U256::from(100u64)), Some(d("0.5")));
    // Точка ниже наименьшей с большим влиянием — вставка как есть, накопленный максимум от неё.
    let low = merge(&down, s(5, "0.9", "0"));
    assert_eq!(at(&low, U256::from(1u64)), Some(d("0.9")));
    // Неубывающая лесенка записи (BTCMT) — выравнивание, как раньше: ниже наименьшей — наименьшая.
    let btcmt = buy("0x62166b8a");
    let mid = trade(
        &btcmt,
        (btcmt[1].amount_in + btcmt[2].amount_in) / U256::from(2u64),
    );
    let up = merge(
        &btcmt,
        HookStep {
            pct: Decimal::ZERO,
            ..mid
        },
    );
    assert!(monotone(&up));
    assert_eq!(
        at(&up, U256::from(1u64)),
        Some(btcmt[0].pct.max(Decimal::ZERO))
    );
}

#[test]
fn n5_no_write_without_change() {
    let cycle = buy("0x62166b8a");
    let old = only_buy(cycle.clone());
    // Отказ выше всех точек стороны ничего не убирает.
    assert!(
        on_failure(
            &old,
            true,
            cycle[2].amount_in + U256::from(1u64),
            "откат".into()
        )
        .is_none()
    );
    // Та же точка сделки второй раз — итог тот же.
    let p = trade(
        &cycle,
        (cycle[1].amount_in + cycle[2].amount_in) / U256::from(2u64),
    );
    let once = on_success(&old, &only_buy(vec![p.clone()]), Some(4)).expect("новая точка");
    assert!(on_success(&once, &only_buy(vec![p]), Some(4)).is_none());
}

#[test]
fn n9_flat_reprobe_clears_fail() {
    let cycle = buy("0x5c1b4d20");
    let old = HookImpact {
        buy_pct: cycle[1].pct,
        buy_fail: Some("откат".into()),
        sell_pct: d("0.1"),
        ..Default::default()
    };
    let new = only_buy(vec![cycle[2].clone()]);
    let flat = only_side(&old, &new, Side::Buy, false);
    assert!(flat.buy.is_empty(), "одно число не становится лесенкой");
    assert_eq!((flat.buy_pct, flat.buy_fail.clone()), (cycle[2].pct, None));
    assert_eq!(flat.sell_pct, d("0.1"));
    let ladder = only_side(&old, &new, Side::Buy, true);
    assert_eq!((ladder.buy.len(), ladder.buy_fail), (1, None));
    // Сторона перемером не измерена — прежняя.
    let failed = HookImpact {
        buy_fail: Some("снова откат".into()),
        ..Default::default()
    };
    assert_eq!(only_side(&old, &failed, Side::Buy, false), old);
}

#[test]
fn n3_side_off_visible() {
    let cycle = buy("0x5c1b4d20");
    let i = HookImpact {
        sell: vec![trade(&cycle, cycle[1].amount_in)].into(),
        buy_fail: Some("NotEnoughLiquidity".into()),
        ..Default::default()
    };
    let pc = PoolCheck {
        venue: VenueId("bsc_dex".into()),
        family: "pancakeswap-infinity-clmm".into(),
        pool: PoolRef::Contract(Address::ZERO),
        asset: Asset {
            ticker: "GAIB".into(),
            bsc_contract: None,
        },
        group: HookGroup::B,
        impact: CheckState::Known(i.clone()),
        checked_at: None,
    };
    assert_eq!(
        pool_sides_off(Some(&pc)),
        vec!["покупка не прошла: NotEnoughLiquidity".to_string()]
    );
    assert!(pool_sides_off(None).is_empty());
    let v = serde_json::to_value(&i).expect("json");
    assert!(v["sell"].is_array() && v["buy"].is_array());
    assert_eq!(v["sell"][0]["coins"], serde_json::json!("1"));
    let back: HookImpact = serde_json::from_value(v).expect("обратно");
    assert_eq!(back, i);
}
