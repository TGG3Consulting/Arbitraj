//! Файл: tests/bsc_dex_hook_ladder_1117.rs
//! Что проверяет: решение «335 (лесенка влияния)» на досъёмке узла 17.11 (tests/data/hook_pools_ticks_2026-11-17,
//! блок 134000292): у крючка 0x32c59d… (GAIB 0x5c1b, BTCMT 0x6216) влияние покупки растёт с суммой.
//! 1. Лесенка покупки по точкам 1 / 20 / 100 USDT (влияние = формула check/hooks_probe.rs impact: наш полный обмен по
//!    карте тиков без крючка против CLQuoter) неубывающая (quote/hook_curve.rs monotone); у продажи — 0 на всех суммах.
//! 2. Котировка с лесенкой (HookCurve::pct_at на входе) на 1, 20, 100 USDT — не выше квотера (до 1 wei округления
//!    Decimal); прежнее правило «влияние, измеренное на 20 USDT, — на все суммы» на 100 USDT было выше квотера
//!    (GAIB на 0,58 %, BTCMT на 2,71 %) — теперь нет.
//! 3. Между точками — с решения 406 (t190) линейная интерполяция между соседними точками (10 USDT — между 1 и 20, 50 USDT
//!    — между 20 и 100): не ниже меньшей точки и ниже прежнего значения ближайшей большей; выше наибольшей точки — «не
//!    измерено на такой сумме» (пул на ней не идёт). Точная формула — tests/bsc_dex_hook_interp_t190.rs.
//! 4. Точный выход (покупка «с конца», HookCurve::settle): на количество монет, которое квотер даёт за 20 USDT, наш
//!    расчёт просит не меньше 20 USDT (до 10⁻¹² — округление точного выхода), а применённое влияние не меньше
//!    влияния на итоговом входе.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/hook_ticks_common, arb::venues::bsc_dex::{check::hooks_probe, quote::{hook_curve, quoter}}.

mod hook_ticks_common;

use std::str::FromStr;
use std::sync::Arc;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use arb::core::types::HookStep;
use arb::venues::bsc_dex::check::hooks_probe::impact;
use arb::venues::bsc_dex::quote::hook_curve::{HookCurve, monotone};
use arb::venues::bsc_dex::quote::quoter::less_hook;
use hook_ticks_common::{Pool, Quote, load, map, ours, pools, quotes};

fn dec(v: U256) -> Decimal {
    Decimal::from_str(&v.to_string()).expect("число")
}

fn keep(pct: Decimal) -> Decimal {
    Decimal::ONE - pct / Decimal::ONE_HUNDRED
}

/// Лесенка стороны пула по записи: точки по возрастанию входа. Ошибок нет.
fn ladder(p: &Pool, qs: &[&Quote]) -> Vec<HookStep> {
    let m = map(p);
    let mut out: Vec<HookStep> = qs
        .iter()
        .map(|q| {
            let o = ours(p, &m, q.z, true, q.amount, p.lp);
            assert!(o.complete);
            HookStep {
                amount_in: q.amount,
                usd: Decimal::from(q.usd),
                pct: impact(o.amount_out, q.remote).expect("влияние"),
                coins: None,
                quoter_out: Some(q.remote),
            }
        })
        .collect();
    out.sort_by_key(|s| s.amount_in);
    out
}

#[test]
fn ladder_quote_not_above_quoter() {
    let all = load();
    let pools = pools(&all);
    let qs = quotes(&all, &pools);
    for (pre, old_over) in [("0x5c1b4d20", "0.5"), ("0x62166b8a", "2.5")] {
        let (id, p) = pools
            .iter()
            .find(|(id, _)| id.starts_with(pre))
            .expect("пул");
        let m = map(p);
        for buy in [true, false] {
            let side: Vec<&Quote> = qs.iter().filter(|q| &q.id == id && q.buy == buy).collect();
            assert_eq!(side.len(), 3);
            let steps = ladder(p, &side);
            assert!(monotone(&steps), "{pre} buy={buy}");
            if !buy {
                assert!(steps.iter().all(|s| s.pct.is_zero()), "продажа без влияния");
            }
            let curve = HookCurve {
                steps: Some(Arc::new(steps.clone())),
                limit: None,
                open: false,
                margin: Some(Decimal::ZERO),
            };
            for q in &side {
                let o = ours(p, &m, q.z, true, q.amount, p.lp).amount_out;
                let pct = curve.pct_at(Decimal::ZERO, q.amount).expect("влияние");
                assert!(
                    dec(o) * keep(pct) <= dec(q.remote) + Decimal::ONE,
                    "{pre} {}",
                    q.usd
                );
                assert!(less_hook(o, pct).expect("вычет") <= q.remote);
            }
            if !buy {
                continue;
            }
            // Прежнее правило: влияние на 20 USDT — на все суммы; на 100 USDT это выше факта.
            let (s20, q100) = (&steps[1], side.iter().find(|q| q.usd == 100).expect("100"));
            let o100 = dec(ours(p, &m, q100.z, true, q100.amount, p.lp).amount_out);
            let over =
                (o100 * keep(s20.pct) - dec(q100.remote)) / dec(q100.remote) * Decimal::ONE_HUNDRED;
            assert!(
                over > Decimal::from_str(old_over).expect("%"),
                "{pre}: {over}"
            );
            // Между точками — интерполяция (решение 406): не ниже меньшей, ниже большей; выше наибольшей — не измерено.
            let at = |usd: u64| curve.pct_at(Decimal::ZERO, steps[0].amount_in * U256::from(usd));
            for (usd, i) in [(10u64, 0usize), (50, 1)] {
                let v = at(usd).expect("измерено");
                assert!(v >= steps[i].pct.max(Decimal::ZERO), "{pre} {usd}: {v}");
                assert!(v < steps[i + 1].pct, "{pre} {usd}: {v}");
            }
            assert!(at(101).is_err());
        }
    }
}

#[test]
fn exact_out_not_better_than_fact() {
    let all = load();
    let pools = pools(&all);
    let qs = quotes(&all, &pools);
    for pre in ["0x5c1b4d20", "0x62166b8a"] {
        let (id, p) = pools
            .iter()
            .find(|(id, _)| id.starts_with(pre))
            .expect("пул");
        let m = map(p);
        let side: Vec<&Quote> = qs.iter().filter(|q| &q.id == id && q.buy).collect();
        let curve = HookCurve {
            steps: Some(Arc::new(ladder(p, &side))),
            limit: None,
            open: false,
            margin: Some(Decimal::ZERO),
        };
        let q20 = side.iter().find(|q| q.usd == 20).expect("20");
        let want = dec(q20.remote);
        let (pct, s) = curve
            .settle(
                Decimal::ZERO,
                |pct| {
                    let out = U256::from_str(&(want / keep(pct)).ceil().normalize().to_string())
                        .map_err(|e| e.to_string())?;
                    let s = ours(p, &m, q20.z, false, out, p.lp);
                    Ok::<_, String>((s.amount_in, s))
                },
                |why| why,
            )
            .expect("сошлось");
        let floor =
            dec(q20.amount) * (Decimal::ONE - Decimal::from_str("0.000000000001").expect("ε"));
        assert!(
            dec(s.amount_in) >= floor,
            "{pre}: просим {} < 20 USDT",
            s.amount_in
        );
        assert!(pct >= curve.pct_at(Decimal::ZERO, s.amount_in).expect("влияние"));
        assert!(dec(s.amount_out) * keep(pct) >= want - Decimal::ONE);
    }
}
