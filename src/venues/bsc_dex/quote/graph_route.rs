//! Файл: venues/bsc_dex/quote/graph_route.rs
//! Что делает: котировка маршрута по локальному графу токенов (решение 80). Строит граф (pools/graph.rs), перечисляет
//! пути USDT ↔ монета (quote/route_paths.rs), оценивает каждый путь нашей математикой (quote/route_calc.rs),
//! делит объём между путями/пулами (сплит, quote/graph_split.rs — решения 38, 80) и собирает котировку ядру через
//! quote/costs.rs с маршрутом по долям (RouteMode::Graph). Фаза 80, PR 80.4 — оценка и сплит; PR 80.5a — врезка.
//! Решение 122 R1: при dex.route_gas_aware = да сплит выбирает доли и пути по чистому выходу (газ пути в USDT —
//! quote/route_gas.rs). Исполнимость маршрута как есть (DexRoute.executable, решение 84; R2/R3 решения 122) —
//! quote/graph_exec.rs: неисполнимый — сигнал/карточка, авто-сделкой не исполняется (exec/trade.rs отказывает).
//! Нативную монету (BNB) не маршрутизирует через V4 / Infinity (решение 62, native_ok). Выбор котировки-на-сделку
//! среди исполнимых маршрутов — merge() (решение 84 вариант Б): graph-режим не исполняет хуже классики.
//! Кто вызывает: quote/mod.rs за переключателем dex.router_mode (merge, quote); тесты.
//! От чего зависит: pools/graph.rs, quote/route_paths.rs, quote/route_calc.rs, quote/graph_legs.rs, quote/graph_split.rs,
//! quote/graph_exec.rs, quote/route_gas.rs, quote/costs.rs, quote/select.rs, quote/via.rs (Via), quote/steps.rs, tokens.rs.
//! Настройки: dex.route_max_hops, dex.route_max_paths, dex.route_max_pools_per_edge, dex.route_gas_aware,
//! dex.native_wrapped (газ в USDT при учёте газа), smart.split_step_pct, smart.max_splits, dex.intermediate_* [bsc_dex];
//! через вызываемые — глубина (решение 73), допуск пула, channel.*, dex.bundle_max_routers.

use std::collections::{HashMap, HashSet};

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{Asset, Quote, RouteMode, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::exec::native;
use super::super::pools::graph;
use super::graph_split::{self, Cand};
use super::route_gas::{self, GasUsdt};
use super::route_paths::RoutePath;
use super::steps::Names;
use super::via::Via;
use super::{Quoting, costs, graph_exec, graph_legs, pick, route_paths};

pub use super::graph_exec::{GraphForm, graph_form};

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
    // Решение 129: граф — один на состояние реестра и блок (pools/graph.rs cached), а не новый на каждую котировку.
    // Свежесть головы проверяет вызывающий (quote/mod.rs); здесь номер блока — только ключ кэша (нет головы — 0).
    let head = v.q.heads.latest().map_or(0, |h| h.number);
    let g = graph::cached(ctx, &v.q.reg, &v.q.tokens, v.usdt_dec, head)?;
    let allowed: HashSet<Address> = v.q.mids.active(ctx).iter().map(|m| m.address).collect();
    let (max_hops, max_paths) = route_paths::limits(ctx)?;
    let mut paths = route_paths::enumerate(&g, v.usdt, v.coin, &allowed, max_hops, max_paths);
    // Решение 62: нативную монету (BNB) не маршрутизируем через V4 / Infinity (CL, Bin) — путь не проверен живой
    // записью; как в классике (quote/mod.rs фильтрует пулы по native::pool_ok). Для ненативных монет — без фильтра.
    if native::is_native(ctx, asset) {
        paths.retain(|p| native_ok(v.coin, p));
    }
    if paths.is_empty() {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            format!("{}: граф не нашёл путей USDT ↔ монета", asset.ticker),
        ));
    }
    let dec = decimals(v, &paths).await?;
    // R1 (решение 122): газ пути в USDT для выбора — только при dex.route_gas_aware = да.
    let gas_aware = route_gas::enabled(ctx);
    let conv = if gas_aware {
        Some(GasUsdt::new(ctx, &v.q.reg, (v.usdt, v.usdt_dec), gas)?)
    } else {
        None
    };
    let mut cands = Vec::new();
    let mut why = Vec::new();
    for p in &paths {
        match graph_legs::legs_for(v.q, p, v.side, v.usdt_dec, &dec) {
            Ok(legs) => {
                let g = match &conv {
                    Some(c) => c.path(&legs)?,
                    None => Decimal::ZERO,
                };
                cands.push(Cand::new(legs, g));
            }
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
    let legs = graph_split::split(ctx, &mut cands, v.side, qty, v.taxes, gas_aware)?;
    // Решения 39 и 146: налоговые правила исполнимости — по налогу ПУЛА каждого шага (Usable.tax_pct), не монеты.
    let exec = graph_exec::executable(ctx, &legs, native::is_native(ctx, asset));
    let mut quote = costs::quote(
        ctx,
        asset,
        v.side,
        qty,
        &legs,
        RouteMode::Graph,
        v.taxes,
        gas,
        names,
    )?;
    if let Some(r) = quote.route.as_mut() {
        r.executable = exec.is_ok();
        r.exec_note = exec.err();
    }
    Ok(quote)
}

/// Выбор котировки-НА-СДЕЛКУ при dex.router_mode = graph (решение 84, вариант Б): среди ИСПОЛНИМЫХ маршрутов — лучший.
/// Классика всегда исполнима; графовый маршрут участвует, только если исполним как есть (DexRoute.executable).
/// Неисполнимый граф-маршрут на сделку НЕ берётся — он «лучший сигнал» (карточка, PR 80.5b), а котировкой-на-сделку
/// остаётся лучший исполнимый, поэтому graph-режим никогда не исполняет хуже/меньше классики. Сбой графа — запись в
/// журнал и откат к классике (ценность «всё видно»). Ошибок нет.
pub async fn merge(
    q: &Quoting,
    classic: Quote,
    graph: GatewayResult<Quote>,
    side: Side,
    usdt_dec: u8,
) -> Quote {
    match graph {
        Ok(g) if g.route.as_ref().is_some_and(|r| r.executable) => {
            pick::pick(q, classic, g, side, usdt_dec).await
        }
        // Неисполнимый как есть граф-маршрут: на сделку не берём. Если он ЛУЧШЕ классики — доносим его как signal_route
        // (лучший найденный сигнал для карточки, решение 84 вариант Б); на сделку остаётся классика.
        Ok(g) => {
            let mut out = classic;
            let best = pick::pick(q, out.clone(), g.clone(), side, usdt_dec).await;
            if best.route.as_ref().map(|r| r.mode) == Some(RouteMode::Graph) {
                out.signal_route = g.route;
            }
            out
        }
        Err(e) => {
            q.ctx
                .log(
                    Level::Warn,
                    "dex.router_graph",
                    format!(
                        "граф-роутер не дал маршрут — котировка по классике: {}",
                        e.message
                    ),
                    None,
                )
                .await;
            classic
        }
    }
}

/// Решение 62: путь не маршрутизирует нативную монету `coin` (BNB) через V4 / Infinity — все хопы, касающиеся монеты,
/// только V2 / V3 (native::pool_ok). Ошибок нет.
pub fn native_ok(coin: Address, path: &RoutePath) -> bool {
    path.steps.iter().all(|s| {
        let touches = s.token_in == coin || s.token_out == coin;
        !touches || native::pool_ok(s.edge.entry.pool.kind)
    })
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
