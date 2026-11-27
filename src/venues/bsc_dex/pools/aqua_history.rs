//! История читается фоном. Позиция и программы записываются вместе, курсор привязан к хешу блока.
use super::{
    aqua::{self, Roles},
    aqua_book::Book,
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
use serde_json::json;
use std::time::Duration;

#[derive(Default)]
struct Snapshot {
    namespace: String,
    last: Option<(u64, B256)>,
    book: Book,
}

async fn sync(
    p: &super::listen::Pools,
    watches: &[super::created::Watch],
    f: &'static Family,
    state: &mut Snapshot,
) -> GatewayResult<()> {
    let ctx = &p.ctx;
    let chain = ctx.chain_id().ok_or_else(|| aqua::bad(ctx))?;
    let tip = white_state::head(ctx, "latest").await?;
    let roles = Roles::checked(ctx, f, &tip.selector()).await?;
    let previous = format!("{chain}:{}:{:#x}:{:#x}", f.id, roles.registry, roles.engine);
    let namespace = format!("aqua:{previous}");
    let store = ctx.journal.db();
    let bad = |s: String| {
        ctx.err(
            ErrorCode::Internal,
            format!("Aqua: журнал открытых программ: {s}"),
        )
    };
    if state.namespace != namespace {
        *state = Snapshot::default();
        let mut rows = venue_state::list(store, ctx.place.id, &namespace, "")
            .await
            .map_err(|e| bad(e.to_string()))?;
        if rows.is_empty()
            && let Some(v) = venue_state::load(store, ctx.place.id, "aqua_history", &previous)
                .await
                .map_err(|e| bad(e.to_string()))?
        {
            // Переход от старой единой записи. Прообразы проверяются тем же путём, что и новые строки.
            #[derive(serde::Deserialize)]
            struct Legacy {
                last: Option<(u64, B256)>,
                book: Book,
            }
            let old: Legacy = serde_json::from_str(&v).map_err(|e| bad(e.to_string()))?;
            rows = old.book.persist().map_err(|e| bad(e.to_string()))?;
            rows.push((
                "cursor".into(),
                serde_json::to_string(&old.last).map_err(|e| bad(e.to_string()))?,
            ));
            venue_state::save_many(store, ctx.place.id, &namespace, rows.clone())
                .await
                .map_err(|e| bad(e.to_string()))?;
        }
        for (key, body) in rows {
            if key == "cursor" {
                state.last = serde_json::from_str(&body).map_err(|e| bad(e.to_string()))?;
            } else if let Some(h) = key.strip_prefix("row:") {
                let h = h.parse().map_err(|_| bad(key))?;
                state
                    .book
                    .restore(h, &body)
                    .ok_or_else(|| bad(format!("повреждённый прообраз {h:#x}")))?;
            }
        }
        let hashes = state.book.hashes().collect();
        super::aqua_attach::connect(p, watches, f, roles, &state.book, &hashes, tip).await?;
        state.namespace = namespace.clone();
    }
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
            state.last = None;
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
        let mut changed = std::collections::BTreeSet::new();
        let mut attach = std::collections::BTreeSet::new();
        for log in &logs {
            if let Some((hash, dirty)) = state.book.observe(roles, log) {
                if dirty {
                    changed.insert(hash);
                }
                // Повторное ship ранее отозванной регистрации будит подписку.
                if dirty || log.topic0() == Some(&topics[0]) {
                    attach.insert(hash);
                }
            }
        }
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
        let delta = state.book.selection(&changed);
        let cursor = Some((end, header.hash));
        let mut rows = delta.persist().map_err(|e| bad(e.to_string()))?;
        rows.push((
            "cursor".into(),
            serde_json::to_string(&cursor).map_err(|e| bad(e.to_string()))?,
        ));
        venue_state::save_many(store, ctx.place.id, &namespace, rows)
            .await
            .map_err(|e| bad(e.to_string()))?;
        state.last = cursor;
        super::aqua_attach::connect(p, watches, f, roles, &state.book, &attach, tip).await?;
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
    let mut state = Snapshot::default();
    loop {
        if !super::discover::enabled(ctx, f).await {
            return;
        }
        if let Err(e) = sync(&p, &watches, f, &mut state).await {
            state = Snapshot::default();
            ctx.log(Level::Warn, "dex.scan", e.message, None).await;
        }
        let Ok(delay) = ctx.int("dex.scan_progress_s") else {
            return;
        };
        tokio::time::sleep(Duration::from_secs(delay.max(1))).await;
    }
}
