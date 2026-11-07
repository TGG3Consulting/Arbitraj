//! Файл: venues/bsc_dex/frozen.rs
//! Что делает: решение 217 — снимок котировки DEX на момент решения о входе в сделку (контракт quote_snapshot,
//! gateway/frozen.rs). В пути сделки — только память: свежесть блока и ленты событий (как у живой котировки — «не
//! торгуем вслепую»), `Arc` записей пулов монеты (весь реестр — при dex.router_mode = graph; pools/registry_frozen.rs),
//! копия разрешений кошелька из памяти (wallet/allow_cache.rs snapshot) и номер блока головы. Индексы замороженного
//! реестра строятся при первой котировке по снимку — уже в фоновом расчёте.
//! Котировка по снимку — та же котировка адаптера (quote/mod.rs priced: маршрут, комиссии пулов, налоги, газ свопов,
//! approve и чаевые) на замороженном реестре. Живыми остаются цена газа (eth_gasPrice, кэш на блок), decimals токенов,
//! налоги и газ разрешений, измеренные пробами, настройки — их наша сделка не сдвигает. Финальная котировка KyberSwap
//! (решение 95) к снимку не относится: снимок — котировка своей математикой по пулам.
//! Кто вызывает: venue.rs (quote_snapshot).
//! От чего зависит: quote/mod.rs (Quoting), pools/registry_frozen.rs, wallet/allow_cache.rs, node/heads.rs, tokens.rs,
//! gateway/frozen.rs, gateway/urgency.rs.
//! Настройки: dex.router_mode [bsc_dex] (какие пулы снимать); feed.stale_after_ms [bsc_dex] (свежесть блока).

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{Asset, OrderBook, Quote, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::frozen::FrozenQuote;
use crate::gateway::urgency::{self, Urgency as Class, scope};

use super::pools::registry::Entry;
use super::quote::Quoting;

/// Снимок котировки DEX стороны сделки.
pub struct FrozenDex {
    /// Окружение котировки с живым реестром и копией разрешений (реестр подменяется замороженным в built).
    base: Quoting,
    entries: Vec<Arc<Entry>>,
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
    let all = ctx.text("dex.router_mode").ok().as_deref() == Some("graph");
    let entries = q.reg.freeze(coin, &mids, usdt, all);
    let mut base = q;
    base.allow = base.allow.snapshot();
    base.frozen_head = Some(head);
    Ok(FrozenDex {
        base,
        entries,
        built: OnceLock::new(),
        asset: asset.clone(),
        side,
        head,
        taken_at: Utc::now(),
    })
}

impl FrozenDex {
    /// Окружение котировки на замороженном реестре (строится один раз, при первой котировке). Ошибок нет.
    fn quoting(&self) -> &Quoting {
        self.built.get_or_init(|| {
            let mut q = self.base.clone();
            q.reg = self.base.reg.frozen_from(self.entries.clone());
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

    /// Текст: сколько пулов снято и блок головы. Ошибок нет.
    fn describe(&self) -> String {
        format!(
            "пулы {} {}: {} записей, блок {}",
            self.base.ctx.venue().0,
            self.asset.ticker,
            self.entries.len(),
            self.head
        )
    }
}
