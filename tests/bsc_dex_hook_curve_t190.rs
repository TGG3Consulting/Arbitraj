//! Файл: tests/bsc_dex_hook_curve_t190.rs
//! Что проверяет: задача t190 — какое правило лесенки влияния крючка ближе к факту: «ближайшая большая точка» (upper),
//! хорда (chord) и правило решения 406 (at: max(хорда, продолжение наклона прошлого участка), не выше upper; на
//! первом участке — хорда). Значение точки — накопленный максимум по ней и меньшим суммам (решение 406).
//! Только запись узла 18.11
//! (tests/data/hook_curve_2026-11-18, блок 134300230): 7 пулов Infinity CL с крючками, квотер на 14 суммах в обе
//! стороны. Влияние крючка на сумме = (наш − квотер) / наш · 100 (check/hooks_probe.rs impact; наш — полный обмен
//! math/swap_cl.rs по тикам записи без крючка). Откат квотера или наш расчёт вне прочитанных слов — «нет котировки».
//! Таблица — в выводе теста (println!): `cargo test --test bsc_dex_hook_curve_t190 -- --nocapture` (в GitHub — шаг
//! «Таблица кривых крючков (t190)» работы tests.yml, часть 0).
//! A. Лесенка hooks.impact_points = 16: первый замер ⌈16/2⌉ = 8 сумм (check/hook_points.rs ladder_usd, first_round) от
//!    signal.probe_usd 10 до 850 USDT, каждая — ближайшая по ln-расстоянию сумма записи: 10, 20, 30, 60, 150, 200, 500,
//!    850. Остальные 8 точек (hooks_refine.rs refine — середины отрезков с наибольшим скачком) зависят от замера и
//!    пришлись бы на 45, 100, 300 — тогда сравнивать было бы почти не на чем; поэтому А — только первый замер.
//!    Лесенка обрывается на первой сумме без котировки (как check/hook_fold.rs judge_side). Сравнение — на суммах
//!    записи вне лесенки (1, 5, 45, 80, 100, 300).
//! B. «Без одной точки»: лесенка — все измеренные суммы стороны, кроме проверяемой внутренней.
//! Δ = правило − факт (факт и правило — не меньше 0, как в коде); Δ < 0 — правило ЗАНИЖАЕТ влияние (опасно); занижение
//! меньше разрешения точек (resolution, 100 / выход квотера) — шум, не считается. ИТОГ — по блокам и правилам.
//! Ассерты — только факты записи: 7 пулов, 196 вызовов квотера (14 сумм × 2 стороны), суммы покупки = USDT·10^18,
//! откаты — ровно те, что в README записи, лесенка А — суммы выше; chord ≤ at ≤ upper (свойство кода); в блоке A на
//! суммах не ниже наименьшей точки лесенки at НЕ занижает (с допуском на разрешение). Ниже наименьшей точки (1, 5 USDT)
//! все правила берут значение наименьшей точки — у CAKE 0xc6104625 (влияние падает с суммой) это занижение до 0,0007
//! п.п. (прикидка вне бота); оно печатается в ИТОГ, ассерта на него нет.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/hook_curve_common (points — точки записи и лесенка А), arb::venues::bsc_dex::quote::hook_curve.

mod hook_curve_common;

use std::collections::BTreeSet;

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use arb::core::types::HookStep;
use arb::venues::bsc_dex::quote::hook_curve::{at, chord, monotone, resolution, upper};
use hook_curve_common::points::{Pt, REVERTS, ladder_a, side, sides, step};

/// Итог правила по блоку, п.п.: сравнений, занижений (больше разрешения точек — resolution), наибольшее занижение,
/// сумма и наибольшее завышение.
#[derive(Clone, Copy, Default)]
struct Stat {
    n: u32,
    under: u32,
    under_max: Decimal,
    over_sum: Decimal,
    over_max: Decimal,
}

impl Stat {
    /// Учесть Δ = правило − факт с допуском шума `tol`. Ошибок нет.
    fn add(&mut self, d: Decimal, tol: Decimal) {
        self.n += 1;
        if d < -tol {
            self.under += 1;
            self.under_max = self.under_max.max(-d);
        }
        if d > Decimal::ZERO {
            self.over_sum += d;
            self.over_max = self.over_max.max(d);
        }
    }
}

/// Сравнение правил на точке p (upper, chord, at): строка печатается, итог — в `st`. `strict` — at не должен занижать
/// на суммах не ниже наименьшей точки (блок A). Ошибок нет; паника — нарушено chord ≤ at ≤ upper или занижение при
/// `strict`.
fn compare(tag: &str, steps: &[HookStep], p: &Pt, st: &mut [Stat; 3], strict: bool) {
    let fact = p.pct.unwrap_or_default().max(Decimal::ZERO);
    let tol = step(p).map(|s| resolution(&s)).unwrap_or_default()
        + steps.iter().map(resolution).max().unwrap_or_default();
    let show = |v: Option<Decimal>| {
        v.map_or("не измерено".into(), |v| {
            format!("{v:.4} (Δ {:+.4})", v - fact)
        })
    };
    let x = p.amount;
    let (u, i, w) = (upper(steps, x), chord(steps, x), at(steps, x));
    if let (Some(u), Some(i), Some(w)) = (u, i, w) {
        assert!(i <= w && w <= u, "{tag} {}: {i} ≤ {w} ≤ {u}", p.usd);
    }
    let inside = steps.first().is_some_and(|f| f.amount_in <= x);
    if let Some(w) = w.filter(|_| strict && inside) {
        assert!(
            w >= fact - tol,
            "{tag} {}: at {w} занижает факт {fact}",
            p.usd
        );
    }
    for (k, v) in [u, i, w].into_iter().enumerate() {
        if let Some(v) = v {
            st[k].add(v - fact, tol);
        }
    }
    println!(
        "{tag} {:>4} | факт {fact:.4} | ближ. большая {} | хорда {} | новое (max(хорда, наклон) ≤ ближ.) {}",
        p.usd,
        show(u),
        show(i),
        show(w)
    );
}

#[test]
fn hook_curve_rules_against_record() {
    let all = sides();
    let sums: Vec<u64> = all[0].2.iter().map(|p| p.usd).collect();
    let reverts: BTreeSet<(&str, bool, u64)> = all
        .iter()
        .flat_map(|(n, b, pts)| {
            pts.iter()
                .filter(|p| p.remote.is_none())
                .map(move |p| (*n, *b, p.usd))
        })
        .collect();
    let want: BTreeSet<(&str, bool, u64)> = REVERTS.into_iter().collect();
    assert_eq!(reverts, want, "откаты квотера — как в README");
    let lad = ladder_a(&sums);
    assert_eq!(lad, vec![10, 20, 30, 60, 150, 200, 500, 850], "лесенка А");
    println!(
        "=== t190: запись 18.11, блок 134300230. Точки: наш (без крючка) / квотер / влияние % ==="
    );
    for (n, b, pts) in &all {
        for p in pts {
            let o = p.ours.map_or("нет".into(), |o| o.to_string());
            let r = p.remote.map_or("откат".into(), |r| r.to_string());
            let pct = p.pct.map_or("нет".into(), |v| v.round_dp(6).to_string());
            println!(
                "{n} {} {:>4} USDT | вход {} | наш {o} | квотер {r} | влияние {pct}",
                side(*b),
                p.usd,
                p.amount
            );
        }
    }
    let mut worst = [[Stat::default(); 3]; 2];
    println!("=== A: лесенка {lad:?} (impact_points = 16, первый замер) ===");
    for (n, b, pts) in &all {
        let steps: Vec<HookStep> = lad
            .iter()
            .map_while(|u| pts.iter().find(|p| p.usd == *u).and_then(step))
            .collect();
        let used: Vec<u64> = steps.iter().map(|s| s.usd.to_u64().unwrap_or(0)).collect();
        println!(
            "--- {n} {}: точки {used:?}, неубывающая {}",
            side(*b),
            monotone(&steps)
        );
        for p in pts
            .iter()
            .filter(|p| p.pct.is_some() && !used.contains(&p.usd))
        {
            compare(
                &format!("A {n} {}", side(*b)),
                &steps,
                p,
                &mut worst[0],
                true,
            );
        }
    }
    println!("=== B: без одной точки (лесенка — остальные измеренные суммы стороны) ===");
    for (n, b, pts) in &all {
        let m: Vec<&Pt> = pts.iter().filter(|p| p.pct.is_some()).collect();
        let inner = m.len().saturating_sub(2);
        for (k, p) in m.iter().enumerate().skip(1).take(inner) {
            let steps: Vec<HookStep> = m
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != k)
                .filter_map(|(_, q)| step(q))
                .collect();
            compare(
                &format!("B {n} {}", side(*b)),
                &steps,
                p,
                &mut worst[1],
                false,
            );
        }
    }
    let names = [
        "ближайшая большая",
        "хорда",
        "новое правило (накопл. максимум; max(хорда, продолжение наклона прошлого участка), не выше ближайшей большей)",
    ];
    for (t, w) in ["A", "B"].iter().zip(worst) {
        for (name, r) in names.iter().zip(w) {
            let mean = r.over_sum / Decimal::from(r.n.max(1));
            println!(
                "ИТОГ {t} | {name}: сравнений {}, занижает {} раз (макс {:.4} п.п.), завышает ср. {mean:.4} / макс {:.4} п.п.",
                r.n, r.under, r.under_max, r.over_max
            );
        }
    }
}
