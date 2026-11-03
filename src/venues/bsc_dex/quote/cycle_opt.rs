//! Файл: venues/bsc_dex/quote/cycle_opt.rs
//! Что делает: ОПТИМАЛЬНЫЙ вход цикла DEX ↔ DEX для сигнала (решение 124, дополнение координатора): вместо одного
//! фиксированного dex_dex.probe_input_base — вход с наибольшей чистой прибылью на отрезке (0, dex_dex.probe_input_base].
//! Газ свопов и чаевые от входа не зависят (замер газа пула — Usable.gas), поэтому максимум чистой прибыли = максимум
//! валовой (выход − вход). Кривая выхода пулов (V2 x·y = k, V3/CL по тикам) вогнута — выход − вход вогнут, у него один
//! максимум: тернарный поиск, число шагов — общая настройка подбора объёма size.refine_steps (решение 66). Вход, на
//! котором шаг не исполним целиком (объём больше пула), считается хуже любого исполнимого — отрезок сжимается к меньшим.
//! size.refine_steps не задана — как раньше: сигнал на самом dex_dex.probe_input_base (без подбора).
//! Исполнение подбирает вход заново в пределах бюджета ядра (core/cycle_plan.rs, тот же поиск size_search.rs).
//! Кто вызывает: quote/cycle_scan.rs.
//! От чего зависит: quote/{cycle, route_calc, path}.rs, ctx.rs.
//! Настройки: size.refine_steps (глобальная), dex_dex.probe_input_base [bsc_dex] (верхняя граница — через вызывающего).

use rust_decimal::Decimal;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::cycle;
use super::path::Path;
use super::route_calc::Leg;

/// Лучший вход на (0, cap]. Принимает: ноги цикла, верхнюю границу входа (базовая). Возвращает: путь на лучшем входе
/// (coins — вход, usdt — выход). Ошибки: как cycle::eval на cap, если ни один вход не считается.
pub fn best(ctx: &Ctx, legs: &[Leg], cap: Decimal) -> GatewayResult<Path> {
    let Ok(steps) = ctx.global_int("size.refine_steps") else {
        return cycle::eval(ctx, legs, cap);
    };
    let gain = |x: Decimal| cycle::eval(ctx, legs, x).ok().map(|p| p.usdt - p.coins);
    let three = Decimal::from(3u8);
    let (mut lo, mut hi) = (Decimal::ZERO, cap);
    for _ in 0..steps {
        let m1 = lo + (hi - lo) / three;
        let m2 = hi - (hi - lo) / three;
        match (gain(m1), gain(m2)) {
            (Some(a), Some(b)) if a < b => lo = m1,
            (Some(_), _) => hi = m2,
            (None, _) => hi = m1,
        }
    }
    let mid = (lo + hi) / Decimal::TWO;
    let mut out: Option<Path> = None;
    for x in [mid, cap] {
        if let Ok(p) = cycle::eval(ctx, legs, x) {
            let better = out
                .as_ref()
                .is_none_or(|b| p.usdt - p.coins > b.usdt - b.coins);
            if better {
                out = Some(p);
            }
        }
    }
    match out {
        Some(p) => Ok(p),
        None => cycle::eval(ctx, legs, cap),
    }
}
