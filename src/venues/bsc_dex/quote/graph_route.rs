//! Файл: venues/bsc_dex/quote/graph_route.rs
//! Что делает: котировка маршрута по локальному графу токенов (решение 80). Строит граф (pools/graph.rs), перечисляет
//! пути USDT ↔ монета (quote/route_paths.rs), оценивает каждый путь нашей математикой (quote/route_calc.rs),
//! делит объём между путями/пулами (сплит, как умный режим — решения 38, 80) и собирает котировку ядру через
//! quote/costs.rs с маршрутом по долям (RouteMode::Graph). Фаза 80, PR 80.4 — оценка и сплит; PR 80.5a — врезка.
//! Выставляет исполнимость маршрута как есть (DexRoute.executable, решение 84): прямая форма (один пул, один хоп)
//! или всё через один Universal Router PancakeSwap (умный путь) — исполнимо; иначе false с причиной (нужен бандл
//! свопов, фаза исполнения) — это сигнал/карточка, авто-сделкой пока не исполняется (exec/trade.rs отказывает).
//! Нативную монету (BNB) не маршрутизирует через V4 / Infinity (решение 62, native_ok). Выбор котировки-на-сделку
//! среди исполнимых маршрутов — merge() (решение 84 вариант Б): graph-режим не исполняет хуже классики.
//! Кто вызывает: quote/mod.rs за переключателем dex.router_mode (merge, quote); тесты.
//! От чего зависит: pools/graph.rs, quote/route_paths.rs, quote/route_calc.rs, quote/graph_legs.rs, quote/costs.rs,
//! quote/select.rs, quote/via.rs (Via), quote/steps.rs (Names), tokens.rs.
//! Настройки: dex.route_max_hops, dex.route_max_paths, dex.route_max_pools_per_edge, smart.split_step_pct,
//! smart.max_splits, dex.intermediate_* [bsc_dex]; через вызываемые — глубина (решение 73), допуск пула.

use std::collections::{HashMap, HashSet};

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{Asset, DexRoute, Quote, RouteMode, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::exec::native;
use super::super::pools::families;
use super::super::pools::graph;
use super::path::Path;
use super::route_paths::RoutePath;
use super::steps::Names;
use super::via::Via;
use super::{Quoting, costs, graph_legs, pick, route_calc, route_paths};

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
    let exec = executable(ctx, &legs);
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

/// Исполним ли маршрут одной транзакцией как есть (решение 84): прямая форма (одна доля, один хоп, любой свой роутер)
/// или всё через один Universal Router PancakeSwap (все хопы всех долей — smart_ok). Иначе — причина (нужен бандл).
fn executable(ctx: &super::super::ctx::Ctx, legs: &[Path]) -> Result<(), String> {
    use super::super::pools::families::smart_ok;
    // Пустой маршрут не исполним (защитный guard; split этого не даёт).
    if legs.is_empty() {
        return Err("пустой маршрут".to_string());
    }
    // Защитно: хоп без своего пути исполнения (tradable=false) — сигнал, авто-сделкой не исполняется (например
    // V3-форк, если он когда-нибудь попадёт в граф). V2-форк теперь tradable — идёт бандлом через свой роутер (ниже).
    if let Some(h) = legs
        .iter()
        .flat_map(|p| p.hops.iter())
        .find(|h| !h.entry.family.tradable)
    {
        return Err(format!(
            "маршрут проходит через {} без своего пути исполнения — это сигнал, авто-сделкой не исполняется",
            h.entry.family.id
        ));
    }
    // Нога глубже 2 хопов существующим исполнением (exec/live.rs — 1–2 хопа) пока не исполняется.
    if legs.iter().any(|p| p.hops.len() > 2) {
        return Err(
            "маршрут содержит ногу глубиной больше 2 хопов — исполнение таких ног ещё не реализовано \
             (решение 84 / exec/live): это сигнал, авто-сделкой не исполняется"
                .to_string(),
        );
    }
    // Форк исполняется ТОЛЬКО своим роутером бандлом (решения 59, 82) — не прямым/умным путём Universal Router. Любой
    // хоп-форк → только через collectable (считает роутеры форков и UR в лимит dex.bundle_max_routers).
    let has_fork = legs
        .iter()
        .flat_map(|p| p.hops.iter())
        .any(|h| h.entry.family.fork);
    if !has_fork {
        if legs.len() == 1 && legs[0].hops.len() == 1 {
            return Ok(());
        }
        if legs
            .iter()
            .all(|p| p.hops.iter().all(|h| smart_ok(ctx, h.entry.family)))
        {
            return Ok(());
        }
    }
    // Мульти-роутер или форк: исполним, если собирается в атомарный бандл свопов (решение 84, exec/bundle.rs) — каждая
    // нога в пределах одного роутера (UR или форка) и всего роутеров ≤ dex.bundle_max_routers. Иначе — сигнал.
    collectable(ctx, legs)
}

/// Исполняющий роутер хопа: у форка — его собственный classic V2 Router (dex.router), иначе — Universal Router своей
/// фабрики (dex.universal_router). Так collectable и exec/bundle.rs считают роутеры одинаково. Ошибки: NotConfigured.
fn exec_router(
    ctx: &super::super::ctx::Ctx,
    f: &super::super::pools::families::Family,
) -> GatewayResult<Address> {
    if f.fork {
        ctx.dex_addr("dex.router", f.id)
    } else {
        ctx.dex_addr("dex.universal_router", f.id)
    }
}

/// Собирается ли маршрут в бандл (решение 84): ни одна нога не пересекает роутеры (как проверяет exec/bundle::
/// single_router/fork_router — у форка свой роутер, dex.router) и всего роутеров (UR и форков) не больше
/// dex.bundle_max_routers. Ошибка — причина несобираемости (нога пересекает роутеры / роутеров больше предела /
/// настройка не задана).
fn collectable(ctx: &super::super::ctx::Ctx, legs: &[Path]) -> Result<(), String> {
    // Не задано → ctx.int вернёт ошибку (настройка L::Positive, значение всегда ≥ 1, если задано).
    let max = ctx.int("dex.bundle_max_routers").map_err(|e| e.message)?;
    let mut routers: Vec<Address> = Vec::new();
    for p in legs {
        let mut leg_router: Option<Address> = None;
        for h in &p.hops {
            let r = exec_router(ctx, h.entry.family).map_err(|e| e.message)?;
            match leg_router {
                None => leg_router = Some(r),
                Some(x) if x != r => {
                    return Err("нога маршрута пересекает роутеры (UR/форк) — бандл такую ногу не собирает (сигнал)".into());
                }
                _ => {}
            }
        }
        if let Some(r) = leg_router
            && !routers.contains(&r)
        {
            routers.push(r);
        }
    }
    if routers.len() as u64 > max {
        return Err(format!(
            "маршрут задействует {} роутеров — больше dex.bundle_max_routers ({max}): бандлом не исполняется (сигнал)",
            routers.len()
        ));
    }
    Ok(())
}

/// Форма исполнимого графового маршрута для боевого пути (exec/trade.rs). Классификация по тем же правилам, что
/// executable: прямая (одна доля, один хоп), один Universal Router PancakeSwap (умный путь), иначе — мульти-роутерный
/// бандл (решение 84).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphForm {
    /// Прямая форма: один пул, один хоп — идёт прямым путём (exec/trade.rs).
    Direct,
    /// Всё через один Universal Router PancakeSwap — идёт умным путём (exec/trade_smart.rs).
    OneUr,
    /// Мульти-роутер — идёт бандлом свопов (exec/bundle_trade.rs).
    Bundle,
}

/// Форма исполнимого графового маршрута. Принимает: маршрут из котировки. Возвращает: форму. Ошибок нет
/// (вызывается только для уже исполнимого маршрута, DexRoute.executable = true).
pub fn graph_form(ctx: &super::super::ctx::Ctx, route: &DexRoute) -> GraphForm {
    // Форк исполняется только своим роутером бандлом (решения 59, 82) — не прямой/умный путь UR. Проверяем до прямой
    // формы: одно-пуловый форк-маршрут тоже идёт бандлом (exec/bundle_trade.rs), а не прямым путём (exec/trade.rs).
    let has_fork = route
        .legs
        .iter()
        .flat_map(|l| l.hops.iter())
        .any(|h| families::classify(ctx, &h.family).is_ok_and(|f| f.fork));
    if has_fork {
        return GraphForm::Bundle;
    }
    if route.legs.len() == 1 && route.legs.first().is_some_and(|l| l.hops.len() == 1) {
        return GraphForm::Direct;
    }
    let one_ur = route.legs.iter().all(|l| {
        l.hops
            .iter()
            .all(|h| families::classify(ctx, &h.family).is_ok_and(|f| families::smart_ok(ctx, f)))
    });
    if one_ur {
        GraphForm::OneUr
    } else {
        GraphForm::Bundle
    }
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
