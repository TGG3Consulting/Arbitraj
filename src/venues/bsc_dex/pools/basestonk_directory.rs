//! Поиск PoolKey по отслеживаемой паре и разрешённым hook, без сканирования всей истории V4.
use super::{
    abi_v4::IStateView,
    basestonk,
    discover::sorted,
    families::{self, Family},
    launch_keys::{self, invalid},
    nami_batch as batch,
    stable_list::Extra,
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, directory::gt_parse::IndexedPool, node::rpc},
};
use alloy_primitives::{Address, Bytes};
use std::collections::{HashMap, HashSet};

async fn family(
    ctx: &Ctx,
    f: &Family,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    size: usize,
    out: &mut Extra,
) -> GatewayResult<()> {
    let roles = launch_keys::roles(ctx, f)?;
    let hooks = roles
        .iter()
        .filter(|r| r.kind == "hook")
        .map(|r| r.address)
        .collect::<Vec<_>>();
    let at = super::nami::block(ctx, "latest").await?;
    let view = ctx.dex_addr("dex.state_view", f.id)?;
    let mut queried = HashSet::new();
    for coin in coins.keys() {
        let mut keys = Vec::new();
        for quote in quotes {
            if *coin == *quote || !queried.insert(sorted(*coin, *quote)) {
                continue;
            }
            for hook in &hooks {
                for fee in basestonk::TIERS {
                    let key = basestonk::key((*coin, *quote), *hook, fee);
                    keys.push((*quote, key));
                }
            }
        }
        for group in keys.chunks(size.max(1)) {
            let calls = group
                .iter()
                .map(|(_, k)| batch::view(&at, view, IStateView::getSlot0Call { poolId: k.id() }))
                .collect::<Vec<_>>();
            let answers = rpc::batch(ctx, &calls).await?;
            out.calls += calls.len();
            if answers.len() != group.len() {
                return Err(invalid(ctx));
            }
            for ((quote, key), res) in group.iter().zip(answers) {
                let raw: Bytes = serde_json::from_str(res?.get()).map_err(|_| invalid(ctx))?;
                let slot = batch::decode(&raw, IStateView::getSlot0Call { poolId: key.id() })?;
                if slot.sqrtPriceX96.is_zero() {
                    continue;
                }
                let row = IndexedPool {
                    dex_id: f.id.into(),
                    pool: format!("{:#x}", key.id()),
                    base: *coin,
                    quote: *quote,
                    base_symbol: coins.get(coin).cloned().unwrap_or_default(),
                    quote_symbol: coins
                        .get(quote)
                        .cloned()
                        .unwrap_or_else(|| format!("{quote:#x}")),
                    reserve_usd: None,
                };
                out.found.entry(*coin).or_default().push(row.clone());
                if coins.contains_key(quote) {
                    out.found.entry(*quote).or_default().push(IndexedPool {
                        base: *quote,
                        quote: *coin,
                        base_symbol: row.quote_symbol,
                        quote_symbol: row.base_symbol,
                        ..row
                    });
                }
            }
        }
    }
    Ok(())
}

pub(super) async fn scan(
    ctx: &Ctx,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    size: usize,
    out: &mut Extra,
) {
    for f in families::all(ctx) {
        if f.variant.basestonk()
            && matches!(ctx.dex_flag("dex.pool_enabled", f.id), Ok(true))
            && let Err(e) = Box::pin(family(ctx, f, coins, quotes, size, out)).await
        {
            out.no_answer += 1;
            out.notes.push(format!("{}: {}", f.id, e.message));
        }
    }
}
