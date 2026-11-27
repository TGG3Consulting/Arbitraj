//! Индикативные размеры в базовом активе. Снимок пробного кошелька служит подбору, выбранный счёт получает свою свежую котировку.
use super::super::parts::Parts;
use super::{
    deployment::Config,
    market::{Snapshot, Ticket},
};
use crate::{
    core::types::{Asset, Side},
    gateway::events::{MarketEvent, SourceId},
};
use alloy::primitives::{Address, B256};
use chrono::Utc;
use rust_decimal::Decimal;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

#[derive(Default)]
struct Book {
    levels: Vec<(Ticket, Decimal, Address)>,
    orders: HashMap<(Address, B256), Ticket>,
}
#[derive(Clone, Default)]
pub(in crate::venues::bsc_dex) struct Cache(Arc<Mutex<Book>>);
impl Cache {
    pub(super) fn indicative(
        &self,
        c: &Config,
        a: Address,
        s: Side,
        qty: Decimal,
    ) -> Option<(Ticket, Decimal)> {
        let b = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (t, cash, _) = b.levels.iter().find(|(t, _, coin)| {
            *coin == a && t.side == s && &t.config == c && t.valid(Utc::now()) && t.qty == qty
        })?;
        Some((t.clone(), *cash))
    }
    pub(in crate::venues::bsc_dex) fn breaks(
        &self,
        p: &Parts,
        asset: &Asset,
        side: Side,
        max: Decimal,
    ) -> Vec<Decimal> {
        let Some(c) = Config::read(&p.ctx) else {
            return vec![];
        };
        if !super::budget::router_ready(&p.ctx, &c.router_origin) {
            return vec![];
        }
        let b = self.0.lock().unwrap_or_else(|e| e.into_inner());
        b.levels
            .iter()
            .filter(|(t, _, coin)| {
                Some(*coin) == asset.contract
                    && t.side == side
                    && t.config == c
                    && t.valid(Utc::now())
                    && t.qty <= max
            })
            .map(|(t, _, _)| t.qty)
            .collect()
    }
    pub(super) fn store(&self, t: Ticket) -> B256 {
        let id = t.order.digest(super::CHAIN_ID, t.config.router);
        let mut b = self.0.lock().unwrap_or_else(|e| e.into_inner());
        b.orders.retain(|_, old| old.valid(Utc::now()));
        b.orders.insert((t.order.user, id), t);
        id
    }
    pub(super) fn take(&self, owner: Address, id: B256) -> Option<Ticket> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .orders
            .remove(&(owner, id))
    }
}
async fn emit(
    p: &Parts,
    out: &mpsc::Sender<MarketEvent>,
    cash: Address,
    levels: Vec<(Ticket, Decimal, Address)>,
) -> bool {
    let mut markets = BTreeMap::new();
    let mut expiry = None;
    for (t, _, a) in &levels {
        let m = markets.entry(*a).or_insert((false, false));
        if t.side == Side::Buy {
            m.0 = true;
        } else {
            m.1 = true;
        }
        let end =
            t.at.timestamp_millis()
                .saturating_add(i64::try_from(t.config.max_age_ms).unwrap_or(i64::MAX));
        expiry = Some(expiry.map_or(end, |e: i64| e.min(end)));
    }
    p.ryze.0.lock().unwrap_or_else(|e| e.into_inner()).levels = levels;
    out.send(MarketEvent::RfqMarkets {
        source: SourceId::new("Ryze"),
        venue: p.ctx.venue(),
        quote: cash,
        markets,
        expires_ms: expiry.unwrap_or(0),
    })
    .await
    .is_ok()
}
pub(in crate::venues::bsc_dex) async fn clear(p: &Parts, out: &mpsc::Sender<MarketEvent>) {
    let empty = p
        .ryze
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .levels
        .is_empty();
    if !empty {
        emit(p, out, Address::ZERO, vec![]).await;
    }
}
pub(in crate::venues::bsc_dex) async fn run(
    p: Parts,
    assets: Vec<Asset>,
    out: mpsc::Sender<MarketEvent>,
) {
    let mut changes = p.ctx.settings.subscribe();
    loop {
        let rev = p.ctx.settings.generation();
        let Some(cfg) = Config::read(&p.ctx) else {
            clear(&p, &out).await;
            if changes
                .recv()
                .await
                .is_err_and(|e| matches!(e, tokio::sync::broadcast::error::RecvError::Closed))
            {
                return;
            }
            continue;
        };
        let interval = p.ctx.int("ryze.books_interval_ms").ok().filter(|v| *v > 0);
        let mut levels = vec![];
        let mut cash = Address::ZERO;
        if interval.is_some()
            && let Some(s) = Snapshot::read(&p, cfg).await
        {
            cash = s.cash;
            let plan = p
                .ctx
                .settings
                .list("ryze.quote_sizes", p.ctx.at())
                .unwrap_or_default();
            for line in &plan {
                let Some((a, q)) = line.split_once('=') else {
                    continue;
                };
                let (Ok(a), Ok(q)) = (a.trim().parse::<Address>(), q.trim().parse::<Decimal>())
                else {
                    continue;
                };
                if q <= Decimal::ZERO || !assets.iter().any(|t| t.contract == Some(a)) {
                    continue;
                }
                for side in [Side::Buy, Side::Sell] {
                    if let Some((t, v)) = s.measure(&p, a, side, q, None).await {
                        levels.push((t, v, a));
                    }
                }
            }
        }
        if rev != p.ctx.settings.generation() {
            continue;
        }
        levels.retain(|(t, _, _)| t.valid(Utc::now()));
        if !emit(&p, &out, cash, levels).await {
            return;
        }
        let delay = async {
            if let Some(ms) = interval {
                tokio::time::sleep(Duration::from_millis(ms)).await
            } else {
                std::future::pending::<()>().await
            }
        };
        tokio::select! {_=delay=>(),v=changes.recv()=>{if v.is_err_and(|e|matches!(e,tokio::sync::broadcast::error::RecvError::Closed)){return;}}}
    }
}
