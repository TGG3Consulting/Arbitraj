//! Опрашивает индикативные уровни отдельно от расчёта кандидата. Отключённый адаптер ожидает изменения настройки.
use super::super::parts::Parts;
use super::market;
use crate::gateway::events::MarketEvent;
use alloy_primitives::Address;
use chrono::Utc;
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

pub(in crate::venues::bsc_dex) async fn run(p: Parts, out: mpsc::Sender<MarketEvent>) {
    let ctx = &p.ctx;
    let mut changes = ctx.settings.subscribe();
    let config = || {
        if !ctx.flag("hashflow.trading").unwrap_or(false) {
            return None;
        }
        let age = ctx.int("hashflow.levels_max_age_ms").ok()?;
        let source = ctx.text("hashflow.source").ok()?;
        let chain = ctx.chain_id()?;
        if age == 0 || source.trim().is_empty() || source == "api" || super::router(chain).is_err()
        {
            return None;
        }
        Some((source, chain, age, p.tokens.usdt(ctx).ok()?))
    };
    let mut previous = None;
    let mut advertised = p.rfqs.populated();
    'poll: loop {
        let current = config();
        if current != previous || (current.is_none() && advertised) {
            if advertised {
                p.rfqs.install("", 0, Instant::now(), None);
                if out
                    .send(MarketEvent::RfqMarkets {
                        source: crate::gateway::events::SourceId::new("Hashflow"),
                        venue: ctx.venue(),
                        quote: Address::ZERO,
                        markets: BTreeMap::new(),
                        expires_ms: 0,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                advertised = false;
            }
            previous.clone_from(&current);
        }
        let Some((source, chain, age, quote)) = current.clone() else {
            if matches!(
                changes.recv().await,
                Err(tokio::sync::broadcast::error::RecvError::Closed)
            ) {
                return;
            }
            continue;
        };
        let start = Instant::now();
        let at = Utc::now().timestamp_millis();
        let duration = Duration::from_millis(age);
        let data = if ctx.vault.has_secret("hashflow.taker_key") {
            let read = tokio::time::timeout(duration, market::book(ctx, &source, chain, None));
            tokio::pin!(read);
            loop {
                tokio::select! {
                    biased;
                    change = changes.recv() => {
                        if matches!(change, Err(tokio::sync::broadcast::error::RecvError::Closed)) { return; }
                        if config() != current {continue 'poll;}
                    }
                    result = &mut read => break result.ok().and_then(Result::ok).map(Arc::new)
                }
            }
        } else {
            None
        };
        if config() != current {
            continue;
        }
        let markets = data
            .as_ref()
            .map(|book| book.markets(quote, &p.tokens))
            .unwrap_or_default();
        p.rfqs.install(&source, chain, start, data);
        if !markets.is_empty() || advertised {
            advertised = !markets.is_empty();
            if out
                .send(MarketEvent::RfqMarkets {
                    source: crate::gateway::events::SourceId::new("Hashflow"),
                    venue: ctx.venue(),
                    quote,
                    markets,
                    expires_ms: at.saturating_add(age.try_into().unwrap_or(i64::MAX)),
                })
                .await
                .is_err()
            {
                return;
            }
        }
        // Смена несвязанной настройки не ускоряет опрос источника с лимитом запросов.
        let deadline = tokio::time::Instant::now() + duration / 2;
        loop {
            tokio::select! {_ = tokio::time::sleep_until(deadline)=>break, change=changes.recv()=>match change {
                Err(tokio::sync::broadcast::error::RecvError::Closed)=>return,
                _ if config() != current =>break,
                _=>{}
            }}
        }
    }
}
