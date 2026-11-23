//! Файл: venues/bsc_dex/quote/route_fit.rs
//! Что делает: решение 294 (Тигран 15.11: потолок сделки — предел, а не обязательный объём; «боту виден сам пул») —
//! путь через промежуточную монету не исполняет объём целиком (route_calc.rs: «шаг маршрута не исполним целиком») →
//! помещающаяся часть одним прямым расчётом по самому пулу, без перебора. Котировка с fillable_qty < qty — тот же
//! контракт, что у одного пула с USDT (quote/direct.rs calc, решение 87): ядро само берёт помещающийся объём (сигнал
//! core/prices.rs, подбор объёма — граница бюджета/глубины, финальный расчёт — решение 111).
//! Как: тем же расчётом, что route_calc.rs (pool_quote::swap), находится шаг-предел и сколько он исполнил до края
//! прочитанной карты тиков (math/swap_cl.rs: amount_in / amount_out частичного обмена, complete = false):
//! - продажа, предел на ПЕРВОМ шаге (монета → промежуточная): монет = исполненный вход / доля налога продажи;
//! - покупка, предел на ПОСЛЕДНЕМ шаге (промежуточная → монета): монет = исполненный выход × доля налога и крючка.
//!
//! Объём — вниз до decimals монеты (fillable не завышается), затем контрольный route_calc::eval на нём; не прошёл
//! (округление на краю) — минус одна наименьшая единица монеты и снова, не больше числа шагов пути. Не нашлось, ноль,
//! не меньше запрошенного — None (прежний NoRoute).
//! Не применяется (None — прежний NoRoute; docs/TASKS.md): предел на другом шаге (нужен обратный расчёт через пул —
//! записи такого пути нет); пул-предел не на математике swap_cl (V3, CL) — Bin, TickBin, Elastic, Algebra: частичный
//! итог не проверен на записи; V2 «выход ≥ резерва» (math/v2.rs amount_in) — у V2 частичного итога нет, а объём
//! «резерв − 1» помещается только при бесконечной цене (решение 294, DECISIONS).
//! route_calc::eval не меняется (на неё опираются graph_split.rs, graph_seq.rs, exec/live.rs); via.rs не меняется —
//! хопы собираются здесь тем же порядком, что Via::calc.
//! Режим graph (merge): неполная классика выбирается наравне с исполнимым графом (pick), неисполнимый граф — только
//! signal_route, ошибка графа NoRoute — классика без записи журнала (иначе Warn на каждую котировку); другая ошибка
//! графа — прежняя запись (graph_route.rs failed).
//! Кто вызывает: quote/via_pools.rs (fit — только при «нет маршрута» и dex.intermediate_partial_fill = да),
//! quote/routing.rs (merge — режим graph, неполная классика, включатель «да»), тесты.
//! От чего зависит: quote/route_calc.rs (eval, Leg, keep), quote/pool_quote.rs, quote/direct.rs (ceil_units),
//! quote/pool_tax.rs, quote/via.rs, quote/pick.rs, num.rs, pools/registry.rs.
//! Настройки: нет своих (включатель читает via_pools.rs).

use rust_decimal::{Decimal, RoundingStrategy};

use crate::core::types::{PoolState, Side};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::mids::Mid;
use super::direct::ceil_units;
use super::path::Path;
use super::pool_quote;
use super::pool_tax;
use super::route_calc::{self, Leg, keep};
use super::via::{Ok2, Via};
use super::{Quoting, pick};
use crate::core::types::{Quote, RouteMode};
use crate::gateway::errors::{ErrorCode, GatewayResult};

impl Via<'_> {
    /// Что делает: помещающаяся часть пути через m в паре пулов (хопы — тем же порядком, что Via::calc).
    /// Принимает: промежуточную, её decimals, P1 («m — USDT»), P2 («монета — m»), количество. Возвращает: путь на
    /// помещающемся объёме (coins < qty) или None — прежний NoRoute. Ошибок нет.
    pub fn fit(&self, m: &Mid, mid_dec: u8, p1: &Ok2, p2: &Ok2, qty: Decimal) -> Option<Path> {
        let leg = |p: &Ok2, (token_in, token_out), (dec_in, dec_out)| Leg {
            entry: p.0.clone(),
            usable: p.1.clone(),
            token_in,
            token_out,
            dec_in,
            dec_out,
        };
        let legs = match self.side {
            Side::Buy => vec![
                leg(p1, (self.usdt, m.address), (self.usdt_dec, mid_dec)),
                leg(p2, (m.address, self.coin), (mid_dec, self.coin_dec)),
            ],
            Side::Sell => vec![
                leg(p2, (self.coin, m.address), (self.coin_dec, mid_dec)),
                leg(p1, (m.address, self.usdt), (mid_dec, self.usdt_dec)),
            ],
        };
        let tax_keep = pool_tax::coin_keep(&legs, self.side, self.taxes);
        fit(&self.q.ctx, &legs, self.side, qty, tax_keep)
    }
}

/// Что делает: помещающаяся часть маршрута (см. шапку). Принимает: окружение, хопы в порядке исполнения, сторону,
/// количество монет, долю налога монеты (как route_calc::eval). Возвращает: путь на помещающемся объёме или None.
/// Ошибок нет (любая ошибка расчёта — None, наружу идёт прежняя ошибка маршрута).
pub fn fit(ctx: &Ctx, legs: &[Leg], side: Side, qty: Decimal, tax_keep: Decimal) -> Option<Path> {
    let coin_dec = match side {
        Side::Buy => legs.last()?.dec_out,
        Side::Sell => legs.first()?.dec_in,
    };
    let unit = Decimal::new(1, u32::from(coin_dec));
    let mut cand = limit(ctx, legs, side, qty, tax_keep)?;
    // Контроль тем же расчётом, что котировка; повторов — не больше числа шагов пути (структура маршрута).
    for _ in 0..legs.len() {
        if cand <= Decimal::ZERO || cand >= qty {
            return None;
        }
        if let Ok(p) = route_calc::eval(ctx, legs, side, cand, tax_keep) {
            return Some(p);
        }
        cand -= unit;
    }
    None
}

/// Что делает: сколько монет помещается по шагу-пределу (см. шапку). Принимает: как fit. Возвращает: монет (вниз до
/// decimals монеты) или None — предел не на поддержанном шаге, пул не на математике swap_cl, шаг исполнен целиком
/// (причина отказа другая), ошибка расчёта. Ошибок нет.
fn limit(ctx: &Ctx, legs: &[Leg], side: Side, qty: Decimal, tax_keep: Decimal) -> Option<Decimal> {
    if tax_keep <= Decimal::ZERO {
        return None;
    }
    let down =
        |v: Decimal, dec: u8| v.round_dp_with_strategy(u32::from(dec), RoundingStrategy::ToZero);
    match side {
        Side::Sell => {
            // Первый шаг принимает монету за вычетом налога продажи (как route_calc.rs sell).
            let leg = legs.first()?;
            if !swap_cl(leg) {
                return None;
            }
            let amount = num::to_units(qty * tax_keep, leg.dec_in)?;
            let s = pool_quote::swap(ctx, &leg.entry, leg.token_in, true, amount).ok()?;
            if s.complete {
                return None;
            }
            let used = num::to_decimal(s.amount_in, leg.dec_in)?;
            Some(down(used / tax_keep, leg.dec_in))
        }
        Side::Buy => {
            // Последний шаг отдаёт монету; доля налога покупки и крючка — на его выходе (как route_calc.rs buy).
            let leg = legs.last()?;
            if !swap_cl(leg) {
                return None;
            }
            let k = tax_keep * keep(leg.usable.hook_pct);
            if k <= Decimal::ZERO {
                return None;
            }
            let pool_out = ceil_units(qty / k, leg.dec_out)?;
            let s = pool_quote::swap(ctx, &leg.entry, leg.token_in, false, pool_out).ok()?;
            if s.complete {
                return None;
            }
            let got = num::to_decimal(s.amount_out, leg.dec_out)?;
            Some(down(got * k, leg.dec_out))
        }
    }
}

/// Пул шага считается математикой swap_cl (V3, CL — частичный итог проверен на записи). Ошибок нет.
fn swap_cl(leg: &Leg) -> bool {
    matches!(
        leg.entry.pool.state,
        PoolState::V3 { .. } | PoolState::Cl { .. }
    )
}

/// Что делает: решение 294, режим graph — выбор между неполной классикой (fillable < qty) и графом при
/// dex.intermediate_partial_fill = да (зовёт routing.rs). Граф Ok и исполним (`ok` = executable и mids_ok, как ветка
/// 271) — pick.rs pick(классика, граф): больший fillable, при равном — итог после газа (тот же вызов и порядок, что в
/// graph_route::merge); граф Ok, но неисполним — классика, граф — signal_route, если pick выбрал бы его (как merge);
/// граф Err с кодом NoRoute — классика БЕЗ записи Warn «dex.router_graph» (merge писал бы её на каждую котировку);
/// другая ошибка графа — та же запись, что у merge (graph_route.rs failed), и классика. Принимает: расчёт,
/// классику, итог графа, признак исполнимости, сторону, decimals USDT. Возвращает: котировку. Ошибок нет.
pub async fn merge(
    q: &Quoting,
    classic: Quote,
    graph: GatewayResult<Quote>,
    ok: bool,
    side: Side,
    usdt_dec: u8,
) -> Quote {
    match graph {
        Ok(g) if ok => pick::pick(q, classic, g, side, usdt_dec).await,
        Ok(g) => {
            let mut out = classic;
            let best = pick::pick(q, out.clone(), g.clone(), side, usdt_dec).await;
            if best.route.as_ref().map(|r| r.mode) == Some(RouteMode::Graph) {
                out.signal_route = g.route;
            }
            out
        }
        // Ошибка графа «нет маршрута» (объём / путей нет) — классика без записи: иначе Warn на каждую котировку монеты,
        // у которой классика всегда неполная. Любая другая (настройки, расчёт) — прежняя запись graph_route.rs failed.
        Err(e) if e.code == ErrorCode::NoRoute => classic,
        Err(e) => {
            super::graph_route::failed(q, &e, &classic).await;
            classic
        }
    }
}
