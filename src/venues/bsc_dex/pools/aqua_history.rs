//! История читается фоном. Позиция и программы записываются вместе, курсор привязан к хешу блока.
use super::{
    aqua::{self, Roles},
    aqua_book::{self, Book},
    families::Family,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    storage::{journal::Level, venue_state},
    venues::bsc_dex::{
        node::rpc,
        quote::white_state::{self, Head},
    },
};
use alloy::rpc::types::Log;
use alloy_primitives::B256;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

#[derive(Default, Serialize, Deserialize)]
struct Snapshot {
    last: Option<(u64, B256)>,
    book: Book,
}

async fn sync(
    p: &super::listen::Pools,
    watches: &[super::created::Watch],
    f: &'static Family,
) -> GatewayResult<()> {
    let ctx = &p.ctx;
    let chain = ctx.chain_id().ok_or_else(|| aqua::bad(ctx))?;
    let tip = white_state::head(ctx, "latest").await?;
    let roles = Roles::checked(ctx, f, &tip.selector()).await?;
    let namespace = format!("{chain}:{}:{:#x}:{:#x}", f.id, roles.registry, roles.engine);
    let store = ctx.journal.db();
    let bad = |s: String| {
        ctx.err(
            ErrorCode::Internal,
            format!("Aqua: журнал открытых программ: {s}"),
        )
    };
    let saved = venue_state::load(store, ctx.place.id, "aqua_history", &namespace)
        .await
        .map_err(|e| bad(e.to_string()))?;
    let mut state: Snapshot = match saved {
        Some(v) => serde_json::from_str(&v).map_err(|e| bad(e.to_string()))?,
        None => Snapshot::default(),
    };
    let mut next = None;
    if let Some((n, h)) = state.last {
        let row: Option<Head> = rpc::call(
            ctx,
            "eth_getBlockByNumber",
            json!([format!("{n:#x}"), false]),
        )
        .await?;
        if row.is_some_and(|r| r.hash == h) {
            next = n.checked_add(1);
        } else {
            state = Snapshot::default();
        }
    }
    aqua_book::publish(chain, roles, state.book.clone());
    for h in state.book.hashes() {
        for w in watches {
            super::created_log::aqua_attach(p, w, f, roles, &state.book, h)
                .await
                .map_err(&bad)?;
        }
    }
    if !ctx.flag("dex.history_scan").unwrap_or(false) {
        return Ok(());
    }
    let mut start = match next {
        Some(n) => n,
        None => super::deploy_block::get(ctx, f, roles.registry).await?,
    };
    let batch = super::scan::batch_size(ctx)?;
    let end_at = tip.number.to::<u64>();
    let topics = aqua::topics();
    let generation = ctx.settings.generation();
    while start <= end_at && ctx.settings.generation() == generation {
        let end = end_at.min(start.saturating_add(batch - 1));
        let tag = format!("{end:#x}");
        let header = white_state::head(ctx, &tag).await?;
        let mut logs: Vec<Log> = rpc::call(
            ctx,
            "eth_getLogs",
            json!([{"address":[roles.registry,roles.engine],"topics":[topics],
            "fromBlock":format!("{start:#x}"),"toBlock":tag}]),
        )
        .await?;
        if logs.iter().any(|l| {
            l.removed
                || l.block_number.is_none_or(|n| n < start || n > end)
                || l.log_index.is_none()
        }) {
            return Err(aqua::bad(ctx));
        }
        logs.sort_by_key(|l| (l.block_number, l.log_index));
        let changed: std::collections::BTreeSet<_> = logs
            .iter()
            .filter_map(|l| state.book.observe(roles, l))
            .collect();
        if let Some((n, h)) = state.last
            && white_state::head(ctx, &format!("{n:#x}")).await?.hash != h
        {
            return Err(aqua::bad(ctx));
        }
        if white_state::head(ctx, &tag).await?.hash != header.hash
            || ctx.settings.generation() != generation
        {
            return Err(aqua::bad(ctx));
        }
        state.last = Some((end, header.hash));
        let body = serde_json::to_string(&state).map_err(|e| bad(e.to_string()))?;
        venue_state::save(store, ctx.place.id, "aqua_history", &namespace, body)
            .await
            .map_err(|e| bad(e.to_string()))?;
        aqua_book::publish(chain, roles, state.book.clone());
        for h in changed {
            for w in watches {
                super::created_log::aqua_attach(p, w, f, roles, &state.book, h)
                    .await
                    .map_err(&bad)?;
            }
        }
        start = end.checked_add(1).ok_or_else(|| aqua::bad(ctx))?;
    }
    Ok(())
}

pub(super) async fn run(
    p: super::listen::Pools,
    watches: Vec<super::created::Watch>,
    f: &'static Family,
) {
    let ctx = &p.ctx;
    loop {
        if !super::discover::enabled(ctx, f).await {
            return;
        }
        if let Err(e) = sync(&p, &watches, f).await {
            ctx.log(Level::Warn, "dex.scan", e.message, None).await;
        }
        let Ok(delay) = ctx.int("dex.scan_progress_s") else {
            return;
        };
        tokio::time::sleep(Duration::from_secs(delay.max(1))).await;
    }
}
