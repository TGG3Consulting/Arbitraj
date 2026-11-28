//! Одна общая каноническая высота, один публичный кадр на разрешённого строителя.
//! Каждый независимый рынок публикуется по мере готовности, без ожидания остальных источников.
use super::super::parts::Parts;
use super::{
    Builder, Head, builder_snapshot, levels,
    markets::{self, Books, Config},
    rpc,
};
use crate::gateway::events::{MarketEvent, SourceId};
use alloy_primitives::{Address, U64};
use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::mpsc;
async fn publish(
    p: &Parts,
    out: &mpsc::Sender<MarketEvent>,
    cash: Address,
    books: &[Arc<Books>],
) -> bool {
    let mut markets = BTreeMap::new();
    let mut expiry = None;
    for book in books {
        for &(coin, buy) in book.sides.keys() {
            let side = markets.entry(coin).or_insert((false, false));
            if buy {
                side.0 = true;
            } else {
                side.1 = true;
            }
        }
        expiry = Some(expiry.map_or(book.frame.timestamp, |t: u64| t.min(book.frame.timestamp)));
    }
    let was = p.slot_markets.replace(books.to_vec());
    if markets.is_empty() && !was {
        return true;
    }
    out.send(MarketEvent::RfqMarkets {
        source: SourceId::new("Fermi"),
        venue: p.ctx.venue(),
        quote: cash,
        markets,
        expires_ms: i64::try_from(expiry.unwrap_or(0).saturating_mul(1000)).unwrap_or(i64::MAX),
    })
    .await
    .is_ok()
}
async fn refresh(p: &Parts, configs: &[Config], rev: u64, out: &mpsc::Sender<MarketEvent>) -> bool {
    let Some(first) = configs.first() else {
        return true;
    };
    let ctx = &p.ctx;
    let timeout = Duration::from_millis(first.timeout);
    let read = async {
        tokio::try_join!(
            rpc::call::<U64>(ctx, "eth_chainId", json!([])),
            rpc::call::<Head>(ctx, "eth_getBlockByNumber", json!(["latest", false]))
        )
    };
    let Ok(Ok((network, head))) = tokio::time::timeout(timeout, read).await else {
        markets::clear(p, out).await;
        return true;
    };
    if network != U64::from(1) || rev != ctx.settings.generation() {
        markets::clear(p, out).await;
        return true;
    }
    let mut work = FuturesUnordered::new();
    for builder in [Builder::Titan, Builder::BuilderNet] {
        let snapshot =
            async move { builder_snapshot(ctx, builder).await.ok().map(Arc::new) }.shared();
        for c in configs
            .iter()
            .filter(|c| c.builder == builder && c.permitted(ctx))
        {
            let snapshot = snapshot.clone();
            let h = &head;
            let job = async move {
                let raw = snapshot.await?;
                if rev != ctx.settings.generation() || !c.permitted(ctx) {
                    return None;
                }
                let book = levels::levels(p, c.clone(), h, &raw, rev).await.ok()?;
                (book.current() && !book.sides.is_empty()).then(|| Arc::new(book))
            };
            work.push(async move { (c, tokio::time::timeout(timeout, job).await.ok().flatten()) });
        }
    }
    let mut books: Vec<_> = p
        .slot_markets
        .current(p)
        .into_iter()
        .filter(|b| b.head.hash == head.hash)
        .collect();
    if work.is_empty() {
        return publish(p, out, first.cash, &[]).await;
    }
    while let Some((c, got)) = work.next().await {
        if rev != ctx.settings.generation() {
            markets::clear(p, out).await;
            return true;
        }
        books.retain(|b| {
            b.current()
                && (b.config.builder != c.builder
                    || !b.config.same_path(c)
                    || b.config.executor != c.executor
                    || b.config.deployment != c.deployment)
        });
        books.extend(got.filter(|b| b.current()));
        if !publish(p, out, first.cash, &books).await {
            return false;
        }
    }
    true
}
pub(in crate::venues::bsc_dex) async fn run(p: Parts, out: mpsc::Sender<MarketEvent>) {
    if p.ctx.chain_id() != Some(1) {
        return;
    }
    let mut changes = p.ctx.settings.subscribe();
    let mut previous = None;
    let mut configs = Vec::new();
    loop {
        let generation = p.ctx.settings.generation();
        if previous != Some(generation) {
            configs = Config::all(&p);
            previous = Some(generation);
        }
        if let Some(first) = configs.first() {
            if !refresh(&p, &configs, generation, &out).await {
                return;
            }
            let pause = Duration::from_millis(first.interval);
            tokio::select! {_=tokio::time::sleep(pause)=>{},changed=changes.recv()=>{
                if matches!(changed,Err(tokio::sync::broadcast::error::RecvError::Closed)){return;}
            }}
        } else {
            markets::clear(&p, &out).await;
            if matches!(
                changes.recv().await,
                Err(tokio::sync::broadcast::error::RecvError::Closed)
            ) {
                return;
            }
        }
    }
}
