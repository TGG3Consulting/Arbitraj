//! Файл: venues/bsc_dex/quote/graph_route.rs
//! Что делает: котировка маршрута по локальному графу токенов (решение 80). Строит граф (pools/graph.rs), перечисляет
//! пути USDT ↔ монета (quote/route_paths.rs), оценивает каждый путь нашей математикой (quote/route_calc.rs),
//! делит объём между путями/пулами (сплит, как умный режим — решения 38, 80) и собирает котировку ядру через
//! quote/costs.rs с маршрутом по долям. Фаза 80, PR 80.4 — оценка пути и сплит (бэклог §7.3).
//! Пока НЕ подключён к quote/mod.rs — врезка за переключателем dex.router_mode и выбор против классика будут на 80.5;
//! метка маршрута временно RouteMode::Smart (RouteMode::Graph и флаг исполнимости — правка стыка на 80.5).
//! Исполнимость мульти-роутерного маршрута одной транзакцией здесь не проверяется — это веха исполнения (решение 84).
//! Кто вызывает: тесты 80.4; позже — quote/mod.rs за dex.router_mode (80.5).
//! От чего зависит: pools/graph.rs, quote/route_paths.rs, quote/route_calc.rs, quote/graph_legs.rs, quote/costs.rs,
//! quote/select.rs, quote/via.rs (Via), quote/steps.rs (Names), tokens.rs.
//! Настройки: dex.route_max_hops, dex.route_max_paths, dex.route_max_pools_per_edge, smart.split_step_pct,
//! smart.max_splits, dex.intermediate_* [bsc_dex]; через вызываемые — глубина (решение 73), допуск пула.

use std::collections::{HashMap, HashSet};

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{Asset, Quote, RouteMode, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::pools::graph;
use super::path::Path;
use super::steps::Names;
use super::via::Via;
use super::{costs, graph_legs, route_calc, route_paths};

/// Путь-кандидат сплита: ноги в порядке исполнения, сколько монет ему уже отдано и его расчёт.
struct Cand {
    legs: Vec<route_calc::Leg>,
    qty: Decimal,
    got: Option<Path>,
}

/// Котировка по графу. Принимает: расчёт путей (монета, сторона, налоги, decimals монеты/USDT), тикеры, монету,
/// количество, цену газа (wei). Возвращает: котировку ядру (доли маршрута, затраты, средняя цена).
/// Ошибки: NotConfigured (настройки графа/сплита), NoRoute (путей нет / не допущены / объём не делится), узел (decimals).
pub async fn quote(
    v: &Via<'_>,
    names: &Names<'_>,
    asset: &Asset,
    qty: Decimal,
    gas: U256,
) -> GatewayResult<Quote> {
    let ctx = &v.q.ctx;
    let g = graph::build(ctx, &v.q.reg, &v.q.tokens, v.usdt_dec)?;
    let allowed: HashSet<Address> = v.q.mids.active(ctx).iter().map(|m| m.address).collect();
    let (max_hops, max_paths) = route_paths::limits(ctx)?;
    let paths = route_paths::enumerate(&g, v.usdt, v.coin, &allowed, max_hops, max_paths);
    if paths.is_empty() {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            format!("{}: граф не нашёл путей USDT ↔ монета", asset.ticker),
        ));
    }
    let dec = decimals(v, &paths).await?;
    let tax_keep = route_calc::keep(if v.side == Side::Buy {
        v.taxes.buy_pct
    } else {
        v.taxes.sell_pct
    });
    let mut cands = Vec::new();
    let mut why = Vec::new();
    for p in &paths {
        match graph_legs::legs_for(v.q, p, v.side, v.usdt_dec, &dec) {
            Ok(legs) => cands.push(Cand {
                legs,
                qty: Decimal::ZERO,
                got: None,
            }),
            Err(w) => why.push(w),
        }
    }
    if cands.is_empty() {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            format!(
                "{}: граф — нет допущенного пути ({})",
                asset.ticker,
                why.join("; ")
            ),
        ));
    }
    let legs = split(ctx, &mut cands, v.side, qty, tax_keep)?;
    costs::quote(
        ctx,
        asset,
        v.side,
        qty,
        &legs,
        RouteMode::Smart,
        v.taxes,
        gas,
        names,
    )
}

/// decimals всех токенов путей — из кэша или узла один раз. Ошибки: узел (decimals).
async fn decimals(
    v: &Via<'_>,
    paths: &[route_paths::RoutePath],
) -> GatewayResult<HashMap<Address, u8>> {
    let mut set: HashSet<Address> = HashSet::new();
    for p in paths {
        for s in &p.steps {
            set.insert(s.token_in);
            set.insert(s.token_out);
        }
    }
    let mut m = HashMap::new();
    for t in set {
        m.insert(t, v.q.tokens.decimals(&v.q.ctx, t).await?);
    }
    Ok(m)
}

/// Жадный сплит объёма между путями-кандидатами (как умный режим, решение 38): объём режется на шаги
/// smart.split_step_pct, каждый шаг отдаётся пути с лучшей предельной отдачей (покупка — меньше USDT за шаг,
/// продажа — больше), не больше smart.max_splits путей. Возвращает: доли (пути). Ошибки: NotConfigured, NoRoute.
fn split(
    ctx: &super::super::ctx::Ctx,
    cands: &mut [Cand],
    side: Side,
    qty: Decimal,
    tax_keep: Decimal,
) -> GatewayResult<Vec<Path>> {
    let max_splits = usize::try_from(ctx.int("smart.max_splits")?).unwrap_or(usize::MAX);
    let step_pct = ctx.dec("smart.split_step_pct")?;
    if step_pct <= Decimal::ZERO || step_pct > Decimal::ONE_HUNDRED || max_splits == 0 {
        return Err(ctx.not_set(
            "smart.split_step_pct",
            "smart.split_step_pct должен быть в (0; 100], smart.max_splits — больше нуля",
        ));
    }
    let unit = qty * step_pct / Decimal::ONE_HUNDRED;
    let mut left = qty;
    while left > Decimal::ZERO {
        let step = unit.min(left);
        let used = cands.iter().filter(|c| c.got.is_some()).count();
        let mut pick: Option<(usize, Path, Decimal)> = None;
        for (i, c) in cands.iter().enumerate() {
            if c.got.is_none() && used >= max_splits {
                continue;
            }
            let Ok(p) = route_calc::eval(ctx, &c.legs, side, c.qty + step, tax_keep) else {
                continue;
            };
            let before = c.got.as_ref().map_or(Decimal::ZERO, |g| g.usdt);
            let delta = p.usdt - before;
            let better = pick.as_ref().is_none_or(|(_, _, d)| match side {
                Side::Buy => delta < *d,
                Side::Sell => delta > *d,
            });
            if better {
                pick = Some((i, p, delta));
            }
        }
        let Some((i, p, _)) = pick else { break };
        cands[i].qty += step;
        cands[i].got = Some(p);
        left -= step;
    }
    let legs: Vec<Path> = cands.iter_mut().filter_map(|c| c.got.take()).collect();
    if legs.is_empty() {
        return Err(ctx.err(ErrorCode::NoRoute, "граф: ни один путь не принял объём"));
    }
    Ok(legs)
}
