//! Файл: venues/bsc_dex/quote/route_cache.rs
//! Что делает: решение 189 (Тигран, 06.11.2026: «строить граф, перебирать и отбирать пути один раз на весь подбор
//! объёма») — набор путей графа берётся из кэша на dex.route_paths_ttl_ms вместо построения графа и перебора на
//! каждой котировке. Причина: граф кэшируется по версии реестра (pools/graph_cache.rs), а версия меняется на каждом
//! свопе любого из тысяч пулов — граф строился почти на каждой из десятков котировок подбора объёма (замер 06.11:
//! ~17 мс на котировку, без запросов к узлу). Набор путей (какие пулы, в какую сторону) за секунды подбора не
//! меняется.
//! Что НЕ кэшируется: состояние пулов. Каждая котировка берёт ЗАПИСЬ каждого пула пути свежей из реестра (резервы,
//! тики, комиссия — как сейчас); пул пропал, стал недостоверным или спящим — путь отбрасывается. Отбор лучших
//! (route_best.rs), сплит, затраты — по-прежнему на каждой котировке, на её объём.
//! Риск: в пределах dex.route_paths_ttl_ms не виден НОВЫЙ путь (новый пул, глубина пула выросла выше порога графа) —
//! для всех котировок монеты, включая финальную перед сделкой (её цифры — по свежему состоянию, исполнение — через
//! предпроверку exec/precheck.rs и симуляцию бандла).
//! Не задана dex.route_paths_ttl_ms — ТОЧНО как раньше (граф и перебор на каждой котировке).
//! Кто вызывает: quote/graph_route.rs.
//! От чего зависит: pools/graph.rs (cached, Cache: paths_get/paths_put), pools/registry.rs, quote/route_paths.rs,
//! quote/route_best.rs, ctx.rs.
//! Настройки: dex.route_paths_ttl_ms [bsc_dex] (необязательная); через вызывающего — dex.route_max_hops,
//! dex.route_max_paths, dex.route_search_max_paths, dex.route_max_pools_per_edge, dex.intermediate_tokens.

use std::collections::HashSet;
use std::time::Duration;

use alloy_primitives::Address;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::pools::graph::{self, Edge, PathsKey, Skeleton};
use super::super::pools::registry::Registry;
use super::super::tokens::Tokens;
use super::route_best;
use super::route_paths::{self, RoutePath, Step};

/// Срок набора путей (dex.route_paths_ttl_ms). None — не задана: как раньше. Ошибки: NotConfigured — задана нулём.
pub fn ttl(ctx: &Ctx) -> GatewayResult<Option<Duration>> {
    match ctx.int("dex.route_paths_ttl_ms") {
        Err(_) => Ok(None),
        Ok(0) => Err(ctx.not_set(
            "dex.route_paths_ttl_ms",
            "dex.route_paths_ttl_ms должна быть больше нуля",
        )),
        Ok(ms) => Ok(Some(Duration::from_millis(ms))),
    }
}

/// Что перебирать: концы пути, разрешённые промежуточные, пределы.
pub struct Search<'a> {
    pub usdt: Address,
    pub coin: Address,
    pub allowed: &'a HashSet<Address>,
    pub max_hops: usize,
    pub max_paths: usize,
    /// dex.route_search_max_paths (решение 187); None — первые max_paths обхода.
    pub cap: Option<usize>,
}

/// Пути USDT → монета: из кэша (свежие записи пулов) или построением графа и перебором (как раньше), с записью в кэш.
/// Принимает: окружение, реестр, токены, знаки USDT, блок головы, что перебирать. Возвращает: пути (может быть пусто).
/// Ошибки: как у graph::cached и ttl.
pub fn paths(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt_dec: u8,
    head: u64,
    s: &Search,
) -> GatewayResult<Vec<RoutePath>> {
    let ttl = ttl(ctx)?;
    let key = key(ctx, s);
    if let Some(t) = ttl
        && let Some(sk) = reg.graph_cache().paths_get(&key, t)
    {
        return Ok(sk.iter().filter_map(|p| fresh(reg, p)).collect());
    }
    let g = graph::cached(ctx, reg, tokens, usdt_dec, head)?;
    let found = match s.cap {
        Some(c) => route_best::enumerate(&g, s.usdt, s.coin, s.allowed, s.max_hops, c),
        None => route_paths::enumerate(&g, s.usdt, s.coin, s.allowed, s.max_hops, s.max_paths),
    };
    if let Some(t) = ttl {
        reg.graph_cache()
            .paths_put(key, found.iter().map(skeleton).collect(), t);
    }
    Ok(found)
}

/// Ключ набора: концы и всё, от чего зависит перебор (пределы, ширина ребра, промежуточные — по порядку). Ошибок нет.
fn key(ctx: &Ctx, s: &Search) -> PathsKey {
    let mut mids: Vec<String> = s.allowed.iter().map(|a| a.to_string()).collect();
    mids.sort();
    let per_edge = ctx
        .int("dex.route_max_pools_per_edge")
        .map(|v| v.to_string())
        .unwrap_or_default();
    let tag = format!(
        "{}|{}|{:?}|{per_edge}|{}",
        s.max_hops,
        s.max_paths,
        s.cap,
        mids.join(",")
    );
    (s.usdt, s.coin, tag)
}

/// Скелет пути: пул и направление каждого шага. Ошибок нет.
fn skeleton(p: &RoutePath) -> Skeleton {
    p.steps
        .iter()
        .map(|st| {
            (
                st.edge.entry.pool.reference.clone(),
                st.token_in,
                st.token_out,
            )
        })
        .collect()
}

/// Путь по скелету со СВЕЖИМИ записями пулов из реестра. None — пул пропал, недостоверен, спит или не в графе.
fn fresh(reg: &Registry, sk: &Skeleton) -> Option<RoutePath> {
    let mut steps = Vec::with_capacity(sk.len());
    for (r, token_in, token_out) in sk {
        let e = reg.get(r)?;
        if !e.valid || e.asleep || !e.family.graphable {
            return None;
        }
        let (a, b) = (e.coin, e.usdt);
        steps.push(Step {
            edge: Edge { entry: e, a, b },
            token_in: *token_in,
            token_out: *token_out,
        });
    }
    Some(RoutePath { steps })
}
