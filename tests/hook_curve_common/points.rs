//! Файл: tests/hook_curve_common/points.rs
//! Что делает: точки записи 18.11 (tests/data/hook_curve_2026-11-18) для задачи t190: по каждому пулу и стороне — 14
//! сумм USDT, вход, выход квотера (None — откат), наш выход без крючка (None — наш расчёт вне прочитанных слов) и
//! влияние крючка % = (наш − квотер) / наш · 100 (check/hooks_probe.rs impact). Суммы — из входов покупки (USDT·10^18,
//! проверяется); у продажи — те же суммы по возрастанию входа (вход продажи серверный ИИ считал по цене slot0 — README).
//! Кто вызывает: tests/bsc_dex_hook_curve_t190.rs.
//! Здесь же — ladder_a: суммы первого замера лесенки hooks.impact_points = 16 (check/hook_points.rs), привязанные к
//! суммам записи.
//! От чего зависит: tests/hook_curve_common (разбор записи), arb::venues::bsc_dex::check::{hooks_probe, hook_points}.

use alloy_primitives::U256;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::collections::BTreeSet;

use arb::core::types::HookStep;
use arb::venues::bsc_dex::check::hook_points::{first_round, ladder_usd};
use arb::venues::bsc_dex::check::hooks_probe::impact;

use super::{load, map, ours, pools, quotes};

/// Пулы записи: имя → начало PoolId (README данных).
pub const NAMES: [(&str, &str); 7] = [
    ("GAIB", "0x5c1b4d20"),
    ("BTCMT", "0x62166b8a"),
    ("BTW", "0x3122a3fa"),
    ("IRYS", "0x63fff184"),
    ("DNA", "0xd0949ad5"),
    ("CAKE_c6104625", "0xc6104625"),
    ("CAKE_c528f2c0", "0xc528f2c0"),
];

/// Откаты квотера по README записи: (пул, покупка, сумма USDT).
pub const REVERTS: [(&str, bool, u64); 7] = [
    ("BTCMT", false, 850),
    ("CAKE_c6104625", false, 200),
    ("CAKE_c6104625", false, 300),
    ("CAKE_c6104625", false, 500),
    ("CAKE_c6104625", true, 500),
    ("CAKE_c6104625", false, 850),
    ("CAKE_c6104625", true, 850),
];

/// Точка записи: сумма USDT, вход, выходы (квотер, наш), влияние %.
pub struct Pt {
    pub usd: u64,
    pub amount: U256,
    pub remote: Option<U256>,
    pub ours: Option<U256>,
    pub pct: Option<Decimal>,
}

/// Стороны пулов записи: (имя, покупка, точки по возрастанию суммы). Паника — запись не по виду.
pub fn sides() -> Vec<(&'static str, bool, Vec<Pt>)> {
    let all = load();
    let ps = pools(&all);
    let qs = quotes(&all, &ps);
    assert_eq!(ps.len(), 7, "пулов");
    assert_eq!(qs.len(), 196, "вызовов квотера");
    let unit = U256::from(10u64).pow(U256::from(18u64));
    let mut sums: Option<Vec<u64>> = None;
    let mut out = Vec::new();
    for (name, pre) in NAMES {
        let (id, p) = ps.iter().find(|(id, _)| id.starts_with(pre)).expect("пул");
        let m = map(p);
        let mut buy: Vec<_> = qs.iter().filter(|q| &q.id == id && q.buy).collect();
        let mut sell: Vec<_> = qs.iter().filter(|q| &q.id == id && !q.buy).collect();
        buy.sort_by_key(|q| q.amount);
        sell.sort_by_key(|q| q.amount);
        assert_eq!((buy.len(), sell.len()), (14, 14), "{name}");
        assert!(
            buy.iter().all(|q| (q.amount % unit).is_zero()),
            "{name}: покупка в целых USDT"
        );
        let usd: Vec<u64> = buy.iter().map(|q| (q.amount / unit).to::<u64>()).collect();
        let first = sums.get_or_insert_with(|| usd.clone());
        assert_eq!(*first, usd, "{name}: те же суммы");
        for (is_buy, list) in [(true, buy), (false, sell)] {
            let pts = list
                .iter()
                .zip(&usd)
                .map(|(q, u)| {
                    let o = ours(p, &m, q.z, q.amount).map(|s| s.amount_out);
                    let pct = o.zip(q.remote).and_then(|(o, r)| impact(o, r));
                    assert!(q.remote.is_none_or(|r| !r.is_zero()), "{name} {u}");
                    Pt {
                        usd: *u,
                        amount: q.amount,
                        remote: q.remote,
                        ours: o,
                        pct,
                    }
                })
                .collect();
            out.push((name, is_buy, pts));
        }
    }
    out
}

/// Точка лесенки из точки записи. Ошибок нет.
pub fn step(p: &Pt) -> Option<HookStep> {
    Some(HookStep {
        amount_in: p.amount,
        usd: Decimal::from(p.usd),
        pct: p.pct?,
        coins: None,
        quoter_out: p.remote,
    })
}

/// Подпись стороны. Ошибок нет.
pub fn side(buy: bool) -> &'static str {
    if buy {
        "USDT→монета"
    } else {
        "монета→USDT"
    }
}

/// Цели первого замера лесенки (А), привязанные к суммам записи по ln-расстоянию. Ошибок нет.
pub fn ladder_a(sums: &[u64]) -> Vec<u64> {
    let f = |d: Decimal| d.to_f64().unwrap_or(f64::NAN);
    let raw = ladder_usd(Decimal::from(10), Decimal::from(850), first_round(16));
    let mut out = BTreeSet::new();
    for t in raw {
        let dist = |s: u64| (f(t) / s as f64).ln().abs();
        let near = sums
            .iter()
            .copied()
            .min_by(|a, b| dist(*a).total_cmp(&dist(*b)));
        out.extend(near);
    }
    out.into_iter().collect()
}
