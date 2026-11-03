//! Файл: venues/bsc_dex/pools/graph_cache.rs
//! Что делает: кэш графа маршрутов (решение 129): граф строится один раз на состояние реестра (версия реестра), блок
//! головы и настройки графа; все котировки и поиск циклов этого состояния берут готовый снимок (Arc). Одно построение
//! за раз (решение 131): параллельные котировки при новой версии реестра не строят один и тот же граф каждая сама.
//! Кто вызывает: quote/graph_route.rs, quote/cycle_scan.rs (через graph::cached); pools/registry.rs (хранит Cache).
//! От чего зависит: graph.rs (build), registry.rs, tokens.rs, ctx.rs.
//! Настройки: dex.route_max_pools_per_edge, dex.depth_band_pct, dex.min_pool_liquidity_usd, dex.depth_hysteresis_pct
//! [bsc_dex] — часть ключа кэша.

use std::sync::{Arc, Mutex};

use alloy_primitives::Address;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::graph::{Graph, build};
use super::registry::Registry;

/// На чём построен граф (решение 129): версия реестра, блок головы, dex.route_max_pools_per_edge, настройки глубины
/// (полоса, порог, гистерезис — текстом), USDT и его decimals.
#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    version: u64,
    head: u64,
    settings: Vec<String>,
    usdt: Option<Address>,
    usdt_dec: u8,
}

/// Кэш последнего графа (решение 129). Один на реестр (pools/registry.rs). Дешёво клонируется. `building` — одно
/// построение за раз: параллельные котировки при новой версии реестра не строят один и тот же граф каждая сама.
#[derive(Clone, Default)]
pub struct Cache {
    slot: Arc<Mutex<Option<Built>>>,
    building: Arc<Mutex<()>>,
}

/// Построенный граф и на чём он построен.
type Built = (Stamp, Arc<Graph>);

/// Граф текущего состояния: из кэша, если с его построения не менялись реестр, блок головы и настройки графа, иначе
/// строится заново (build) и кладётся в кэш. Принимает: окружение, реестр, токены, знаки USDT, номер блока головы.
/// Возвращает: граф. Ошибки: как у build.
pub fn cached(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt_dec: u8,
    head: u64,
) -> GatewayResult<Arc<Graph>> {
    // Версия — ДО чтения реестра: запись во время построения даст новую версию, и следующий вызов перестроит граф.
    let version = reg.version();
    // Значения как прочитаны (не задана — пусто; build сам скажет, какой нет).
    let dec = |n: &str| {
        ctx.dec(n)
            .map(|d| d.normalize().to_string())
            .unwrap_or_default()
    };
    let stamp = Stamp {
        version,
        head,
        settings: vec![
            ctx.int("dex.route_max_pools_per_edge")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            dec("dex.depth_band_pct"),
            dec("dex.min_pool_liquidity_usd"),
            dec("dex.depth_hysteresis_pct"),
        ],
        usdt: tokens.usdt(ctx).ok(),
        usdt_dec,
    };
    let cache = reg.graph_cache();
    let hit = || {
        cache
            .slot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .filter(|(st, _)| *st == stamp)
            .map(|(_, g)| g.clone())
    };
    if let Some(g) = hit() {
        return Ok(g);
    }
    // Одно построение за раз; дождавшийся берёт граф, построенный другим, если он на том же состоянии. Вызывается из
    // счёта котировки вне рабочих потоков tokio (quote/cpu.rs) — ожидание замка рантайм не держит.
    let _one = cache.building.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(g) = hit() {
        return Ok(g);
    }
    let slot = &cache.slot;
    let g = Arc::new(build(ctx, reg, tokens, usdt_dec)?);
    let mut m = slot.lock().unwrap_or_else(|p| p.into_inner());
    // Не затираем более новый граф, построенный параллельно.
    if m.as_ref().is_none_or(|(st, _)| st.version <= version) {
        *m = Some((stamp, g.clone()));
    }
    Ok(g)
}
