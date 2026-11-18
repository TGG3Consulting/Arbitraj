//! Файл: tests/bsc_dex_hook_interp_t190.rs
//! Что проверяет: решение 405 (задача t190, Тигран 18.11.2026) — влияние крючка между точками лесенки — линейная
//! интерполяция по входу, а не значение ближайшей большей точки. Точки — запись узла 17.11
//! (tests/data/hook_pools_ticks_2026-11-17, блок 134000292; крючок 0x32c59d…, GAIB 0x5c1b, BTCMT 0x6216; покупка
//! 1 / 20 / 100 USDT — наш полный обмен по карте тиков против CLQuoter, формула check/hooks_probe.rs impact). Квотера
//! на промежуточных суммах в записи нет — интерполяция проверяется как правило на реальных точках, не как факт крючка.
//! 1. В точке — её значение; ниже наименьшей — значение наименьшей; выше наибольшей — «не измерено».
//! 2. На 10 и 50 USDT — v₀ + (v₁ − v₀)·(x − a₀)/(a₁ − a₀) по соседним точкам (сверка независимым расчётом в Decimal);
//!    прежнее правило (upper — ближайшая большая) на 50 USDT давало значение точки 100 USDT — новое строго меньше.
//! 3. Интерполяция неубывает по сумме (1…100 USDT).
//! 4. Точный выход «с конца» (HookCurve::settle) на количество монет ~50 USDT (наш обмен по записи пула с влиянием
//!    интерполяции): применённое влияние не меньше влияния на итоговом входе и меньше прежнего (точка 100 USDT).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/hook_ticks_common, arb::venues::bsc_dex::{check::hooks_probe, quote::hook_curve}.

mod hook_ticks_common;

use std::str::FromStr;
use std::sync::Arc;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use arb::core::types::HookStep;
use arb::venues::bsc_dex::check::hooks_probe::impact;
use arb::venues::bsc_dex::quote::hook_curve::{HookCurve, at, monotone, upper};
use hook_ticks_common::{Pool, Quote, load, map, ours, pools, quotes};

fn dec(v: U256) -> Decimal {
    Decimal::from_str(&v.to_string()).expect("число")
}

fn keep(pct: Decimal) -> Decimal {
    Decimal::ONE - pct / Decimal::ONE_HUNDRED
}

/// Пул записи и его лесенка покупки (1 / 20 / 100 USDT, по возрастанию входа) с направлением обмена.
fn buy_ladder(pre: &str) -> (Pool, bool, Vec<HookStep>) {
    let all = load();
    let ps = pools(&all);
    let qs = quotes(&all, &ps);
    let (id, p) = ps
        .into_iter()
        .find(|(id, _)| id.starts_with(pre))
        .expect("пул");
    let m = map(&p);
    let side: Vec<&Quote> = qs.iter().filter(|q| q.id == id && q.buy).collect();
    assert_eq!(side.len(), 3);
    let z = side[0].z;
    let mut out: Vec<HookStep> = side
        .iter()
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
    (p, z, out)
}

/// Независимый расчёт интерполяции (п. 2 шапки) по точкам i и i+1, значения — не меньше 0.
fn expect_lerp(s: &[HookStep], i: usize, x: U256) -> Decimal {
    let (a0, a1) = (dec(s[i].amount_in), dec(s[i + 1].amount_in));
    let (v0, v1) = (s[i].pct.max(Decimal::ZERO), s[i + 1].pct.max(Decimal::ZERO));
    v0 + (v1 - v0) * ((dec(x) - a0) / (a1 - a0))
}

#[test]
fn interp_between_recorded_points() {
    let eps = Decimal::from_str("0.000000000000000001").expect("ε");
    for pre in ["0x5c1b4d20", "0x62166b8a"] {
        let (_, _, s) = buy_ladder(pre);
        assert!(monotone(&s), "{pre}");
        assert!(
            s[0].pct < s[1].pct && s[1].pct < s[2].pct,
            "{pre}: влияние растёт"
        );
        // 1. Точки, ниже наименьшей, выше наибольшей.
        for p in &s {
            assert_eq!(at(&s, p.amount_in), Some(p.pct.max(Decimal::ZERO)));
        }
        assert_eq!(at(&s, U256::from(1u64)), Some(s[0].pct.max(Decimal::ZERO)));
        assert_eq!(at(&s, s[2].amount_in + U256::from(1u64)), None);
        // 2. 10 USDT — между 1 и 20, 50 USDT — между 20 и 100.
        let usd = |u: u64| s[0].amount_in * U256::from(u);
        for (u, i) in [(10u64, 0usize), (50, 1)] {
            let v = at(&s, usd(u)).expect("измерено");
            let want = expect_lerp(&s, i, usd(u));
            assert!((v - want).abs() <= eps, "{pre} {u}: {v} против {want}");
            assert_eq!(upper(&s, usd(u)), Some(s[i + 1].pct), "{pre} {u}: прежнее");
            assert!(v < s[i + 1].pct, "{pre} {u}: интерполяция ниже прежнего");
        }
        // 3. Неубывание по сумме: 101 сумма от точки 1 USDT до точки 100 USDT.
        let (a0, a2) = (s[0].amount_in, s[2].amount_in);
        let mut last = Decimal::MIN;
        for k in 0..=100u64 {
            let x = a0 + (a2 - a0) * U256::from(k) / U256::from(100u64);
            let v = at(&s, x).expect("измерено");
            assert!(v >= last, "{pre} {k}: {v} < {last}");
            last = v;
        }
    }
}

#[test]
fn settle_with_interp_not_better_than_ladder() {
    for pre in ["0x5c1b4d20", "0x62166b8a"] {
        let (p, z, s) = buy_ladder(pre);
        let m = map(&p);
        let curve = HookCurve {
            steps: Some(Arc::new(s.clone())),
            limit: None,
            open: false,
        };
        let x50 = s[0].amount_in * U256::from(50u64);
        let pct50 = at(&s, x50).expect("измерено");
        let want = dec(ours(&p, &m, z, true, x50, p.lp).amount_out) * keep(pct50);
        let (pct, sw) = curve
            .settle(
                Decimal::ZERO,
                |pct| {
                    let out = U256::from_str(&(want / keep(pct)).ceil().normalize().to_string())
                        .map_err(|e| e.to_string())?;
                    let sw = ours(&p, &m, z, false, out, p.lp);
                    Ok::<_, String>((sw.amount_in, sw))
                },
                |why| why,
            )
            .expect("сошлось");
        let on_x = curve.pct_at(Decimal::ZERO, sw.amount_in).expect("влияние");
        assert!(pct >= on_x, "{pre}: применено {pct} < {on_x} на входе");
        assert!(pct < s[2].pct, "{pre}: {pct} не ниже прежнего {}", s[2].pct);
        assert!(
            sw.amount_in > s[1].amount_in && sw.amount_in < s[2].amount_in,
            "{pre}"
        );
        assert!(
            dec(sw.amount_out) * keep(pct) >= want - Decimal::ONE,
            "{pre}"
        );
    }
}
