//! Файл: venues/bsc_dex/quote/graph_fit.rs
//! Что делает: решение 300 (живой случай SIREN 15.11) — лучший маршрут графа неисполним как есть (graph_exec.rs
//! executable вернул причину) → лучший ИСПОЛНИМЫЙ маршрут графа: тот же сплит (graph_split.rs; при заданной
//! dex.route_search_max_paths — сначала отбор route_best.rs) повторяется на подмножествах путей-кандидатов, каждое из
//! которых исполнимо по построению:
//! - S1 — все шаги пути исполняет один Universal Router PancakeSwap (smart_ok, не форк) — одна транзакция;
//! - S2 — все шаги пути — шаги исполнителя кошелька (graph_executor.rs all_via: вид шага + свой контракт пула);
//!   нативная монета — только у исполнителя v2 (как executable);
//! - S3 — все шаги пути tradable и внутри ОДНОГО роутера (как graph_exec.rs collectable) — только если бандл возможен
//!   (send::planned_bundle_ok) и задан dex.bundle_max_routers; число роутеров итога проверяет executable.
//!
//! Во всех подмножествах путь через промежуточную с пулом с налогом стороны > 0 — только V2 (решение 39, как executable).
//! Подмножество пустое или равно всему набору (итог был бы тем же неисполнимым) — не считается. Каждый итог снова
//! проходит executable и costs::quote (газ); лучший среди исполнимых — pick.rs (больше объём, затем чистый итог после
//! газа), как у существующего выбора. Дополнительных сплитов — не больше числа подмножеств (3); при заданной
//! dex.route_search_max_paths перед каждым — ещё отбор route_best::best на подмножестве. Газ approve и чаевые в
//! сравнение не входят (их добавляет quote/mod.rs после выбора) — как у graph_route::merge.
//! Где считается: только котировка решения/исполнения (Quoting.exec_quote, ставит quote/mod.rs) и настройка «да»:
//! признак final (gateway::quote_stage::is_final) — финальная котировка ядра (core/profit.rs final_calc, final_fillable),
//! быстрая предпроверка (core/precheck_fast.rs → final_calc), точки окна открытых сделок (core/window_deal_run.rs
//! final_if); признак exec (gateway::patience::is_exec) — котировка продажи перед ордером (core/trade_ops.rs
//! sell_quote_of) и своя котировка сделки «лучший из двух» (best_of/trade.rs). Подбор объёма — без повторов.
//! Кто вызывает: quote/graph_route.rs (quote_fit — best), quote/routing.rs (enabled, parts, merge, finish).
//! От чего зависит: quote/{graph_split, graph_exec, graph_executor, route_best, costs, pick, pool_tax}.rs,
//! pools/{families, exec_kind}.rs, exec/executor/{entry, mod}.rs, send/mod.rs (planned_bundle_ok).
//! Настройки: dex.graph_exec_fallback [bsc_dex] (решение 300; не задана / «нет» — как раньше); через вызываемые —
//! smart.*, dex.route_*, dex.bundle_max_routers, exec.executor_address, channel.*.

use rust_decimal::Decimal;

use crate::core::types::{Asset, Quote, RouteMode, Side};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::exec::executor::{self, entry};
use super::super::exec::native;
use super::super::pools::exec_kind;
use super::super::pools::families::smart_ok;
use super::graph_split::{self, Cand};
use super::route_calc::Leg;
use super::steps::Names;
use super::via::Via;
use super::{Quoting, costs, graph_exec, pick, pool_tax, route_best};
use alloy_primitives::U256;

/// Пути-кандидаты до отбора (ноги и газ пути в USDT) — снимок для повторных сплитов.
pub type Pool = Vec<(Vec<Leg>, Decimal)>;

/// Лучший исполнимый маршрут графа: котировка, подмножество, лучший найденный (неисполнимый) маршрут.
pub struct Fit {
    pub quote: Quote,
    pub subset: &'static str,
    pub signal: Quote,
}

/// Что нужно повторному сплиту (как у graph_route::quote).
pub struct Ask<'a, 'b> {
    pub v: &'a Via<'b>,
    pub names: &'a Names<'b>,
    pub asset: &'a Asset,
    pub qty: Decimal,
    pub gas: U256,
    /// dex.route_search_max_paths (None — отбора нет) и dex.route_max_paths.
    pub cap: Option<usize>,
    pub max_paths: usize,
    pub gas_aware: bool,
}

/// Включено ли (решение 300): котировка решения/исполнения и dex.graph_exec_fallback = да. Ошибок нет: не задана — нет.
pub fn enabled(q: &Quoting) -> bool {
    q.exec_quote && q.ctx.flag("dex.graph_exec_fallback").unwrap_or(false)
}

/// Снимок кандидатов до отбора и сплита. Ошибок нет.
pub fn snapshot(cands: &[Cand]) -> Pool {
    cands.iter().map(|c| (c.legs.clone(), c.gas_usdt)).collect()
}

/// Решение 39 по налогу пула (как executable): путь через промежуточную — пул с налогом стороны > 0 только V2.
fn tax_ok(legs: &[Leg]) -> bool {
    legs.len() < 2
        || legs
            .iter()
            .all(|l| pool_tax::v2_only(&l.entry, &l.usable).is_ok())
}

/// Правило пути подмножества (Send + Sync: расчёт идёт в потоке счёта, quote/cpu.rs).
type Rule<'a> = Box<dyn Fn(&[Leg]) -> bool + Send + Sync + 'a>;

/// Подмножества, исполнимые по построению: имя и правило пути. Неприменимые (нет бандла, исполнителя для BNB) — нет.
fn subsets(ctx: &Ctx, native: bool) -> Vec<(&'static str, Rule<'_>)> {
    let mut out: Vec<(&'static str, Rule<'_>)> = vec![(
        "S1 — один Universal Router PancakeSwap (одна транзакция)",
        Box::new(move |l: &[Leg]| {
            l.iter()
                .all(|h| !h.entry.family.fork && smart_ok(ctx, h.entry.family))
        }),
    )];
    if !native || executor::native_capable(ctx) {
        out.push((
            "S2 — через исполнитель кошелька (одна транзакция)",
            Box::new(move |l: &[Leg]| {
                l.iter().all(|h| {
                    exec_kind::via_executor(ctx, h.entry.family) && entry::fits_ok(ctx, &h.entry)
                })
            }),
        ));
    }
    let bundle = super::super::send::planned_bundle_ok(ctx).is_ok()
        && ctx.int("dex.bundle_max_routers").is_ok();
    if bundle {
        out.push((
            "S3 — бандл свопов, каждая доля внутри одного роутера",
            Box::new(move |l: &[Leg]| {
                let r: Vec<_> = l
                    .iter()
                    .map(|h| graph_exec::exec_router(ctx, h.entry.family).ok())
                    .collect();
                l.iter()
                    .all(|h| h.entry.family.tradable && !(native && h.entry.family.fork))
                    && r.first()
                        .is_some_and(|f| f.is_some() && r.iter().all(|x| x == f))
            }),
        ));
    }
    out
}

/// Лучший исполнимый маршрут графа (см. шапку). Принимает: что нужно сплиту, снимок кандидатов, лучшую (неисполнимую)
/// котировку графа. Возвращает: Fit или None — ни одно подмножество не дало исполнимого маршрута. Ошибок нет: сбой
/// расчёта подмножества — подмножество не берётся (настройки уже прочитаны основным сплитом).
pub async fn best(a: &Ask<'_, '_>, pool: &Pool, signal: &Quote) -> Option<Fit> {
    let (ctx, v) = (&a.v.q.ctx, a.v);
    let native = native::is_native(ctx, a.asset);
    let mut found: Option<(Quote, &'static str)> = None;
    for (name, rule) in subsets(ctx, native) {
        let sub: Vec<Cand> = pool
            .iter()
            .filter(|(l, _)| tax_ok(l) && rule(l))
            .map(|(l, g)| Cand::new(l.clone(), *g))
            .collect();
        if sub.is_empty() || sub.len() == pool.len() {
            continue;
        }
        let Some(q) = one(a, sub, native) else {
            continue;
        };
        found = match found {
            None => Some((q, name)),
            // Равенство — раньше найденное (порядок подмножеств), как pick: первый аргумент при равенстве.
            Some((b, n)) => {
                let w = pick::pick(v.q, b.clone(), q.clone(), v.side, v.usdt_dec).await;
                if w.route != b.route {
                    Some((q, name))
                } else {
                    Some((b, n))
                }
            }
        };
    }
    found.map(|(quote, subset)| Fit {
        quote,
        subset,
        signal: signal.clone(),
    })
}

/// Один повторный сплит подмножества: отбор (если задан), сплит, исполнимость, затраты. None — не дал исполнимого.
fn one(a: &Ask<'_, '_>, mut sub: Vec<Cand>, native: bool) -> Option<Quote> {
    let (ctx, v) = (&a.v.q.ctx, a.v);
    if a.cap.is_some() {
        sub = route_best::best(ctx, sub, v.side, a.qty, v.taxes, a.max_paths).ok()?;
    }
    let legs = graph_split::split(ctx, &mut sub, v.side, a.qty, v.taxes, a.gas_aware).ok()?;
    graph_exec::executable(ctx, &legs, native).ok()?;
    // costs::quote ставит executable = да, exec_note = нет — маршрут исполним (проверено выше).
    costs::quote(
        ctx,
        a.asset,
        v.side,
        a.qty,
        &legs,
        RouteMode::Graph,
        v.taxes,
        a.gas,
        a.names,
    )
    .ok()
}

/// Разделить итог graph_route::quote_fit на котировку графа и лучший исполнимый. Ошибок нет.
pub fn parts(r: GatewayResult<(Quote, Option<Fit>)>) -> (GatewayResult<Quote>, Option<Fit>) {
    match r {
        Ok((g, f)) => (Ok(g), f),
        Err(e) => (Err(e), None),
    }
}

/// Котировка лучшего исполнимого на сделку: подпись выбора (Quote.pick — в хронологию сделки) и лучший найденный
/// неисполнимый маршрут в signal_route, если он лучше (pick), как у merge. Ошибок нет.
pub async fn finish(q: &Quoting, f: Fit, side: Side, usdt_dec: u8) -> Quote {
    let mut won = f.quote;
    let note = f
        .signal
        .route
        .as_ref()
        .and_then(|r| r.exec_note.clone())
        .unwrap_or_default();
    let w = pick::pick(q, won.clone(), f.signal.clone(), side, usdt_dec).await;
    if w.route.as_ref().is_some_and(|r| !r.executable) {
        won.signal_route = f.signal.route;
    }
    won.pick = Some(format!(
        "граф: на сделку — лучший исполнимый маршрут графа ({}); лучший найденный неисполним: {note}",
        f.subset
    ));
    won
}

/// Решение 300: выбранная котировка (классика после merge) против лучшего исполнимого графа — pick (при равенстве
/// остаётся выбранная). Выиграл граф — finish. Нет Fit — выбранная как есть. Ошибок нет.
pub async fn merge(q: &Quoting, out: Quote, fit: Option<Fit>, side: Side, usdt_dec: u8) -> Quote {
    let Some(f) = fit else { return out };
    let w = pick::pick(q, out.clone(), f.quote.clone(), side, usdt_dec).await;
    if w.route == out.route {
        return out;
    }
    finish(q, f, side, usdt_dec).await
}
