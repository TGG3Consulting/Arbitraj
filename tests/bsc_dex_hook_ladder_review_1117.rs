//! Файл: tests/bsc_dex_hook_ladder_review_1117.rs
//! Что проверяет: ревью второго прохода решения «335 (лесенка влияния)» на досъёмке узла 17.11
//! (tests/data/hook_pools_ticks_2026-11-17, блок 134000292; крючок 0x32c59d…: влияние покупки GAIB и BTCMT растёт с
//! суммой). Лесенка — из записи (наш полный обмен по карте тиков без крючка против CLQuoter, формула
//! check/hooks_probe.rs impact). Правила — чистые функции, которыми живёт путь сделки (exec/hook_trade.rs,
//! exec/trade_probe.rs hooks_again → check/hook_fold.rs) и цикл крючков (check/hook_points.rs, check/hooks_head.rs):
//! - Б1: цикл измерил 1 и 20 USDT, сделка на 100 USDT: строгая котировка — «не измерено»; оценка (open) — значение
//!   верхней точки; после пробы на 100 USDT (точка сделки) лесенка сделки покрывает 100 и даёт факт — наш расчёт не
//!   выше квотера; прежнее «не измерено → пул не идёт» в сделке не случается.
//! - Б2: отказ пробы на 100 USDT — точки ≥ 100 убраны, 1 и 20 работают, продажа не тронута; отказ на 1 USDT
//!   (наименьшая точка) — пул «не прошла»; без лесенки — хранилище не трогается.
//! - Н1: факт сделки на 20 USDT идёт в расчёт напрямую (лесенка хранилища без него): сумма 20 покрыта фактом сделки,
//!   50 — нет (ближайшая точка 100 — из хранилища), значит на 50 — ещё проба.
//! - Н2: лесенка с убыванием (значения записи: BTCMT 20 USDT выше GAIB 100 USDT) — ниже наименьшей точки «не
//!   измерено», внутри — наибольшее.
//! - Н6: без лесенки (одно число — влияние на 20 USDT, как до лесенки) — то же число на всех суммах; точка сделки не
//!   ниже этого числа.
//! - Н3: доуточнение — середина отрезка с наибольшим скачком (20–100 USDT), у продажи (влияние 0) — нет точек.
//! - ladder_usd, first_round, widen (верх продажи по худшей цене полосы), merge (накопленный максимум).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/hook_ticks_common, arb::venues::bsc_dex::{check::{hook_fold, hook_points, hooks_probe},
//! quote::{hook_curve, quoter}}.

mod hook_ticks_common;

use std::str::FromStr;
use std::sync::Arc;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use arb::core::types::{CheckState, HookImpact, HookStep};
use arb::venues::bsc_dex::check::hook_fold::{covered, local, on_failure, on_success, refine};
use arb::venues::bsc_dex::check::hook_points::{first_round, ladder_usd, widen};
use arb::venues::bsc_dex::check::hooks_probe::impact;
use arb::venues::bsc_dex::quote::hook_curve::{HookCurve, at, merge};
use arb::venues::bsc_dex::quote::quoter::less_hook;
use hook_ticks_common::{Pool, Quote, load, map, ours, pools, quotes};

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).expect("число")
}

/// Точки стороны пула по записи (1 / 20 / 100 USDT, по возрастанию входа). Ошибок нет.
fn side(p: &Pool, qs: &[Quote], id: &str, buy: bool) -> Vec<HookStep> {
    let m = map(p);
    let mut out: Vec<HookStep> = qs
        .iter()
        .filter(|q| q.id == id && q.buy == buy)
        .map(|q| {
            let o = ours(p, &m, q.z, true, q.amount, p.lp);
            assert!(o.complete);
            HookStep {
                amount_in: q.amount,
                usd: Decimal::from(q.usd),
                pct: impact(o.amount_out, q.remote).expect("влияние"),
                quoter_out: Some(q.remote),
            }
        })
        .collect();
    out.sort_by_key(|s| s.amount_in);
    assert_eq!(out.len(), 3);
    out
}

/// Пул по префиксу ключа: запись, покупка, продажа, вызовы квотера пула. Ошибок нет.
fn pool(pre: &str) -> (Pool, Vec<HookStep>, Vec<HookStep>, Vec<Quote>) {
    let all = load();
    let ps = pools(&all);
    let qs = quotes(&all, &ps);
    let (id, p) = ps
        .into_iter()
        .find(|(id, _)| id.starts_with(pre))
        .expect("пул");
    let (b, s) = (side(&p, &qs, &id, true), side(&p, &qs, &id, false));
    let mine = qs.into_iter().filter(|q| q.id == id).collect();
    (p, b, s, mine)
}

fn curve(steps: Vec<HookStep>, open: bool) -> HookCurve {
    HookCurve {
        steps: Some(Arc::new(steps)),
        limit: None,
        open,
    }
}

#[test]
fn b1_trade_above_top_gets_fact() {
    for pre in ["0x5c1b4d20", "0x62166b8a"] {
        let (p, buy, _, qs) = pool(pre);
        let (low, top) = (buy[..2].to_vec(), &buy[2]);
        let x = top.amount_in;
        // Котировка (строго): 100 USDT выше наибольшей точки цикла — «не измерено».
        assert!(curve(low.clone(), false).pct_at(Decimal::ZERO, x).is_err());
        // Сделка до пробы (open): оценка — значение верхней точки (20 USDT), пул не выпадает.
        let est = curve(low.clone(), true)
            .pct_at(Decimal::ZERO, x)
            .expect("оценка");
        assert_eq!(est, low[1].pct.max(Decimal::ZERO));
        assert!(!covered(&low, &[], x));
        // Проба на сумме сделки → лесенка сделки: покрыто, влияние = факт на 100 USDT, наш не выше квотера.
        let fresh = vec![top.clone()];
        let l = local(&low, Decimal::ZERO, &fresh);
        assert!(covered(&l, &fresh, x));
        let pct = curve(l, false).pct_at(Decimal::ZERO, x).expect("факт");
        assert_eq!(pct, top.pct);
        let q = qs.iter().find(|q| q.buy && q.usd == 100).expect("100 USDT");
        let o = ours(&p, &map(&p), q.z, true, q.amount, p.lp).amount_out;
        assert!(less_hook(o, pct).expect("вычет") <= q.remote, "{pre}");
    }
}

#[test]
fn b2_failure_cuts_side_only() {
    let (_, buy, sell, _) = pool("0x62166b8a");
    let old = HookImpact {
        buy_pct: buy[2].pct,
        sell_pct: sell[2].pct,
        buy: buy.clone(),
        sell: sell.clone(),
    };
    match on_failure(&old, true, buy[2].amount_in, "нет ликвидности".into()) {
        Some(CheckState::Known(i)) => {
            assert_eq!(i.buy, buy[..2].to_vec());
            assert_eq!(i.buy_pct, buy[1].pct);
            assert_eq!((i.sell, i.sell_pct), (sell.clone(), old.sell_pct));
        }
        x => panic!("ожидалась обрезанная лесенка: {x:?}"),
    }
    assert!(matches!(
        on_failure(&old, true, buy[0].amount_in, "откат".into()),
        Some(CheckState::Failed { .. })
    ));
    let flat = HookImpact {
        buy_pct: buy[1].pct,
        sell_pct: Decimal::ZERO,
        buy: Vec::new(),
        sell: Vec::new(),
    };
    assert!(on_failure(&flat, true, buy[2].amount_in, "откат".into()).is_none());
    assert!(on_success(&flat, &old).is_none());
    let merged = on_success(&old, &old).expect("лесенка");
    assert_eq!(merged.buy, buy);
}

#[test]
fn n1_trade_fact_used_directly() {
    let (_, buy, _, _) = pool("0x62166b8a");
    let stored = vec![buy[0].clone(), buy[2].clone()];
    let fresh = vec![buy[1].clone()];
    let l = local(&stored, Decimal::ZERO, &fresh);
    assert!(covered(&l, &fresh, buy[1].amount_in));
    assert_eq!(at(&l, buy[1].amount_in), Some(buy[1].pct));
    let fifty = buy[0].amount_in * U256::from(50u64);
    assert!(!covered(&l, &fresh, fifty));
}

#[test]
fn n2_decreasing_ladder_below_low_not_measured() {
    let (_, gaib, _, _) = pool("0x5c1b4d20");
    let (_, btcmt, _, _) = pool("0x62166b8a");
    // Значения записи: 0,744 % (BTCMT, 20 USDT) на меньшем входе, 0,674 % (GAIB, 100 USDT) на большем — убывание
    // (вход обоих — USDT, суммы сравнимы).
    let steps = vec![btcmt[1].clone(), gaib[2].clone()];
    assert!(steps[0].pct > steps[1].pct && steps[0].amount_in < steps[1].amount_in);
    assert_eq!(at(&steps, steps[0].amount_in - U256::from(1u64)), None);
    assert_eq!(at(&steps, steps[1].amount_in), Some(steps[0].pct));
    // Неубывающая — ниже наименьшей точки берётся наименьшая (как раньше).
    assert_eq!(
        at(&btcmt, U256::from(1u64)),
        Some(btcmt[0].pct.max(Decimal::ZERO))
    );
}

#[test]
fn n6_flat_as_before() {
    let (_, buy, _, _) = pool("0x5c1b4d20");
    let flat = buy[1].pct;
    let c = HookCurve::default();
    for s in &buy {
        assert_eq!(c.pct_at(flat, s.amount_in), Ok(flat));
    }
    let l = local(&[], flat, &[buy[0].clone(), buy[2].clone()]);
    assert_eq!(l[0].pct, flat);
    assert_eq!(l[1].pct, buy[2].pct);
}

#[test]
fn n3_refine_and_ladder_sums() {
    let (_, buy, sell, _) = pool("0x5c1b4d20");
    let mid = refine(&buy, 1);
    assert_eq!(mid.len(), 1);
    assert!(mid[0] > d("44.7") && mid[0] < d("44.8"));
    assert!(refine(&sell, 3).is_empty());
    assert_eq!(
        ladder_usd(d("1"), d("100"), 3),
        vec![d("1"), d("10"), d("100")]
    );
    assert_eq!(ladder_usd(d("5"), d("5"), 4), vec![d("5")]);
    assert_eq!(
        (
            first_round(1),
            first_round(2),
            first_round(5),
            first_round(6)
        ),
        (1, 2, 3, 3)
    );
    assert_eq!(widen(d("100"), d("20")), d("125"));
    // merge: накопленный максимум — точка ниже соседа слева поднимается до него.
    let low = HookStep {
        pct: Decimal::ZERO,
        ..buy[2].clone()
    };
    let m = merge(&buy[..2], low);
    assert_eq!(m[2].pct, buy[1].pct);
}
