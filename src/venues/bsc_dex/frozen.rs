//! Файл: venues/bsc_dex/frozen.rs
//! Что делает: решение 217 — снимок котировки DEX на момент решения о входе в сделку (контракт quote_snapshot,
//! gateway/frozen.rs). В пути сделки — только память: свежесть блока и ленты событий (как у живой котировки — «не
//! торгуем вслепую»), `Arc` записей пулов, нужных расчёту по ЭТОЙ монете (pools/registry_frozen.rs: пулы монеты,
//! «промежуточная — USDT», «WBNB — USDT» для цены газа; при dex.router_mode = graph — ещё пулы путей, которые живой кэш
//! графа уже нашёл для монеты, quote/route_cache.rs lookup — десятки записей, никакого полного реестра), вердикты
//! глубины этих пулов, копия разрешений кошелька из памяти (wallet/allow_cache.rs snapshot) и номер блока головы.
//! Индексы замороженного реестра строятся при первой котировке по снимку — уже в фоновом расчёте; пути графа
//! закреплены в нём (pools/graph_cache.rs pinned) — граф заново не строится и не спорит за dex.quote_cpu_max.
//! Graph-режим, а путей монеты в кэше на момент снимка нет (dex.route_paths_ttl_ms не задана или набор устарел): снимок
//! — тот же минимальный набор, что у прямого режима; граф замороженной котировки строится по этим десяткам пулов
//! (прямые пути и пути через промежуточные), а классика — как у живой (merge). Это нижняя оценка живой котировки (живой
//! граф мог найти путь через другие пулы) — отмечается в тексте снимка (describe). Почему не «не посчитано»: без
//! dex.route_paths_ttl_ms кэша путей нет никогда, и расчёт сделок в graph-режиме не работал бы вовсе; строить граф
//! или перебирать пути в пути сделки нельзя (сотни мс).
//! Котировка по снимку — та же котировка адаптера (quote/mod.rs priced: маршрут, комиссии пулов, налоги, газ свопов,
//! approve и чаевые) на замороженном реестре. Решение 237: цена газа закреплена на момент снимка (quote/gas.rs pinned —
//! последняя известная из памяти; её не было — котировка по снимку вернёт StaleData, живая не подставляется). Живыми остаются decimals токенов, налоги и газ разрешений,
//! измеренные пробами (меняются только новым замером), настройки — их наша сделка не сдвигает. Финальная котировка KyberSwap
//! (решение 95) к снимку не относится: снимок — котировка своей математикой по пулам.
//! Кто вызывает: venue.rs (quote_snapshot).
//! От чего зависит: quote/mod.rs (Quoting), pools/registry_frozen.rs, wallet/allow_cache.rs, node/heads.rs, tokens.rs,
//! gateway/frozen.rs, gateway/urgency.rs.
//! Настройки: dex.router_mode [bsc_dex] (снимать ли пути графа); dex.native_wrapped (пулы газа); через
//! quote/route_cache.rs — dex.route_paths_ttl_ms, dex.route_max_hops, dex.route_max_paths, dex.route_search_max_paths,
//! dex.route_max_pools_per_edge (ключ набора путей); feed.stale_after_ms [bsc_dex] (свежесть блока).

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{Asset, OrderBook, Quote, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::frozen::FrozenQuote;
use crate::gateway::urgency::{self, Urgency as Class, scope};

use alloy_primitives::Address;

use super::pools::graph::{PathsKey, Skeleton};
use super::pools::registry::Registry;
use super::pools::registry_frozen::{Snap, Want};
use super::quote::{Quoting, route_best, route_cache, route_paths};

/// Снимок котировки DEX стороны сделки.
pub struct FrozenDex {
    /// Окружение котировки с живым реестром и копией разрешений (реестр подменяется замороженным в built).
    base: Quoting,
    snap: Snap,
    /// Пути графа монеты из живого кэша на момент снимка (ключ кэша и скелеты); None — прямой режим или путей не было.
    paths: Option<(PathsKey, Arc<Vec<Skeleton>>)>,
    /// Graph-режим: был ли он на момент снимка (для текста).
    graph: bool,
    built: OnceLock<Quoting>,
    asset: Asset,
    side: Side,
    head: u64,
    taken_at: DateTime<Utc>,
}

/// Что делает: снимок (см. шапку). Принимает: окружение котировки адаптера (Parts::quoting), монету, сторону.
/// Возвращает: FrozenDex. Ошибки: StaleData — нет свежего блока или лента событий отстаёт; NotConfigured — тикер USDT;
/// NoRoute — у монеты нет адреса контракта.
pub fn take(q: Quoting, asset: &Asset, side: Side) -> GatewayResult<FrozenDex> {
    let ctx = &q.ctx;
    let head = q.heads.fresh(ctx)?.number;
    q.lag.check(ctx)?;
    let usdt = q.tokens.usdt(ctx)?;
    let coin = asset.bsc_contract.ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            format!("{}: нет адреса контракта в BSC", asset.ticker),
        )
    })?;
    let mids: Vec<_> = q.mids.active(ctx).iter().map(|m| m.address).collect();
    let graph = ctx.text("dex.router_mode").ok().as_deref() == Some("graph");
    let paths = if graph {
        graph_paths(&q, usdt, coin, &mids)
    } else {
        None
    };
    let snap = q.reg.freeze(&Want {
        coin,
        mids: &mids,
        usdt,
        wbnb: ctx.addr("dex.native_wrapped").ok(),
        paths: paths.as_ref().map_or(&[], |p| p.1.as_slice()),
    });
    // Решение 237: цена газа — закреплена на момент снимка (из памяти, без узла).
    let gas = q.gas.pinned();
    let mut base = q;
    base.gas = gas;
    base.allow = base.allow.snapshot();
    base.frozen_head = Some(head);
    Ok(FrozenDex {
        base,
        snap,
        paths,
        graph,
        built: OnceLock::new(),
        asset: asset.clone(),
        side,
        head,
        taken_at: Utc::now(),
    })
}

/// Пути графа монеты, уже найденные живым кэшем (quote/route_cache.rs lookup — только чтение кэша, без графа и
/// перебора). Принимает: окружение котировки, USDT, монету, промежуточные. Возвращает: ключ и скелеты; None — настройки
/// графа не заданы (живая котировка графа тоже не работает) или набора в кэше нет. Ошибок нет.
fn graph_paths(
    q: &Quoting,
    usdt: Address,
    coin: Address,
    mids: &[Address],
) -> Option<(PathsKey, Arc<Vec<Skeleton>>)> {
    let ctx = &q.ctx;
    let allowed: HashSet<Address> = mids.iter().copied().collect();
    let (max_hops, max_paths) = route_paths::limits(ctx).ok()?;
    let search = route_cache::Search {
        usdt,
        coin,
        allowed: &allowed,
        max_hops,
        max_paths,
        cap: route_best::search_cap(ctx).ok()?,
    };
    route_cache::lookup(ctx, &q.reg, &search).ok().flatten()
}

impl FrozenDex {
    /// Окружение котировки на замороженном реестре (строится один раз, при первой котировке). Ошибок нет.
    fn quoting(&self) -> &Quoting {
        self.built.get_or_init(|| {
            let mut q = self.base.clone();
            q.reg = Registry::frozen_from(&self.snap, self.paths.clone());
            q
        })
    }
}

#[async_trait]
impl FrozenQuote for FrozenDex {
    /// Котировка на объём по пулам снимка (quote/mod.rs priced). Ошибки: как у котировки адаптера, кроме свежести.
    async fn quote(&self, qty: Decimal) -> GatewayResult<Quote> {
        let class = urgency::is_set().unwrap_or(Class::Quote);
        scope(class, self.quoting().priced(&self.asset, self.side, qty)).await
    }

    /// У DEX стакана нет. Ошибок нет.
    fn book(&self) -> Option<&OrderBook> {
        None
    }

    /// Когда снят. Ошибок нет.
    fn taken_at(&self) -> DateTime<Utc> {
        self.taken_at
    }

    /// Текст: сколько пулов снято, пути графа и блок головы. Ошибок нет.
    fn describe(&self) -> String {
        let graph = match (&self.paths, self.graph) {
            (Some((_, p)), _) => format!(", путей графа {}", p.len()),
            (None, true) => {
                ", путей графа в кэше на момент решения нет — граф по пулам монеты и промежуточных (может быть занижено)"
                    .to_string()
            }
            (None, false) => String::new(),
        };
        format!(
            "пулы {} {}: {} записей{graph}, блок {}",
            self.base.ctx.venue().0,
            self.asset.ticker,
            self.snap.entries.len(),
            self.head
        )
    }
}
