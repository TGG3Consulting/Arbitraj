//! Файл: tests/bsc_dex_hook_curve_t190.rs
//! Что проверяет: задача t190 — какое правило лесенки влияния крючка ближе к факту: «ближайшая большая точка» (upper,
//! правило до решения 405) или линейная интерполяция (at, решение 405). Только запись узла 18.11
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
//! Δ = правило − факт (факт и правило — не меньше 0, как в коде); Δ < 0 — правило ЗАНИЖАЕТ влияние (опасно).
//! Ассерты — только факты записи: 7 пулов, 196 вызовов квотера (14 сумм × 2 стороны), суммы покупки = USDT·10^18,
//! откаты — ровно те, что в README записи, лесенка А — суммы выше.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/hook_curve_common (points — точки записи), arb::venues::bsc_dex::{check::hook_points,
//! quote::hook_curve}.

mod hook_curve_common;

use std::collections::BTreeSet;

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use arb::core::types::HookStep;
use arb::venues::bsc_dex::check::hook_points::{first_round, ladder_usd};
use arb::venues::bsc_dex::quote::hook_curve::{at, monotone, upper};
use hook_curve_common::points::{Pt, REVERTS, sides, step};

/// Итог правила: (число занижений, наибольшее занижение п.п.). Строка сравнения печатается. Ошибок нет.
fn compare(tag: &str, steps: &[HookStep], p: &Pt, worst: &mut [(u32, Decimal); 2]) {
    let fact = p.pct.unwrap_or_default().max(Decimal::ZERO);
    let show = |v: Option<Decimal>| {
        v.map_or("не измерено".into(), |v| {
            format!("{v:.4} (Δ {:+.4})", v - fact)
        })
    };
    let (u, i) = (upper(steps, p.amount), at(steps, p.amount));
    for (k, v) in [u, i].into_iter().enumerate() {
        if let Some(v) = v.filter(|v| *v < fact) {
            worst[k].0 += 1;
            worst[k].1 = worst[k].1.max(fact - v);
        }
    }
    println!(
        "{tag} {:>4} | факт {fact:.4} | ближ. большая {} | интерп. {}",
        p.usd,
        show(u),
        show(i)
    );
}

/// Цели первого замера лесенки (А), привязанные к суммам записи по ln-расстоянию. Ошибок нет.
fn ladder_a(sums: &[u64]) -> Vec<u64> {
    let f = |d: Decimal| d.to_f64().unwrap_or(f64::NAN);
    let raw = ladder_usd(Decimal::from(10), Decimal::from(850), first_round(16));
    let mut out = BTreeSet::new();
    for t in raw {
        let near = sums.iter().min_by(|a, b| {
            let da = (f(t) / *a as f64).ln().abs();
            let db = (f(t) / *b as f64).ln().abs();
            da.total_cmp(&db)
        });
        out.extend(near.copied());
    }
    out.into_iter().collect()
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
    assert_eq!(
        reverts,
        REVERTS.into_iter().collect(),
        "откаты квотера — как в README"
    );
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
    let mut worst = [[(0u32, Decimal::ZERO); 2]; 2];
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
            compare(&format!("A {n} {}", side(*b)), &steps, p, &mut worst[0]);
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
            compare(&format!("B {n} {}", side(*b)), &steps, p, &mut worst[1]);
        }
    }
    for (t, w) in ["A", "B"].iter().zip(worst) {
        println!(
            "ИТОГ {t}: ближайшая большая занижает {} раз, наибольшее занижение {:.4} п.п.; интерполяция занижает {} раз, наибольшее {:.4} п.п.",
            w[0].0, w[0].1, w[1].0, w[1].1
        );
    }
}

/// Подпись стороны. Ошибок нет.
fn side(buy: bool) -> &'static str {
    if buy {
        "USDT→монета"
    } else {
        "монета→USDT"
    }
}
