//! Обход allPairs канонической фабрики. Каждая найденная пара отдельно подтверждается через getPair.
use super::{
    families::{self, Family},
    family_variant::Variant,
    integral_size::{self, ISize, Roles},
    nami_batch as batch,
    stable_list::Extra,
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, directory::gt_parse::IndexedPool, node::rpc},
};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
use std::collections::HashMap;
async fn rows(ctx: &Ctx, calls: &[(&'static str, Value)]) -> GatewayResult<Vec<Bytes>> {
    let raw = rpc::batch(ctx, calls).await?;
    if raw.len() != calls.len() {
        return Err(integral_size::bad(ctx));
    }
    raw.into_iter()
        .map(|r| serde_json::from_str(r?.get()).map_err(|_| integral_size::bad(ctx)))
        .collect()
}
async fn registry(
    ctx: &Ctx,
    f: &Family,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    out: &mut Extra,
) -> GatewayResult<()> {
    let at = super::nami::block(ctx, "latest").await?;
    let roles = Roles::read(ctx, f.id)?;
    roles.factory_code(ctx, &at).await?;
    let factory = roles.factory;
    let [n] = batch::read(
        ctx,
        [batch::view(&at, factory, ISize::allPairsLengthCall {})],
    )
    .await?;
    out.calls += 1;
    let count = u64::try_from(batch::decode(&n, ISize::allPairsLengthCall {})?)
        .map_err(|_| integral_size::bad(ctx))?;
    let batch_size = ctx.int("dex.verify_batch")?.max(1);
    let mut i = 0u64;
    while i < count {
        let end = i.saturating_add(batch_size).min(count);
        let calls = (i..end)
            .map(|i| batch::view(&at, factory, ISize::allPairsCall { i: U256::from(i) }))
            .collect::<Vec<_>>();
        let pools = rows(ctx, &calls)
            .await?
            .into_iter()
            .map(|x| batch::decode(&x, ISize::factoryCall {}))
            .collect::<GatewayResult<Vec<_>>>()?;
        let calls = pools
            .iter()
            .flat_map(|p| {
                [
                    batch::view(&at, *p, ISize::token0Call {}),
                    batch::view(&at, *p, ISize::token1Call {}),
                ]
            })
            .collect::<Vec<_>>();
        let tokens = rows(ctx, &calls).await?;
        out.calls += 3 * pools.len();
        for (pool, t) in pools.iter().zip(tokens.chunks_exact(2)) {
            let a = batch::decode(&t[0], ISize::token0Call {})?;
            let b = batch::decode(&t[1], ISize::token1Call {})?;
            for (base, quote) in [(a, b), (b, a)] {
                if let Some(symbol) = coins.get(&base)
                    && quotes.contains(&quote)
                {
                    out.found.entry(base).or_default().push(IndexedPool {
                        dex_id: f.id.into(),
                        pool: format!("{pool:#x}"),
                        base,
                        quote,
                        base_symbol: symbol.clone(),
                        quote_symbol: coins
                            .get(&quote)
                            .cloned()
                            .unwrap_or_else(|| format!("{quote:#x}")),
                        reserve_usd: None,
                    });
                }
            }
        }
        i = end;
    }
    Ok(())
}
pub(super) async fn scan(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut out = Extra::default();
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| f.variant == Variant::IntegralSize)
    {
        if families::classify(ctx, f.id).is_err() {
            continue;
        }
        if let Err(e) = registry(ctx, f, coins, quotes, &mut out).await {
            out.no_answer += 1;
            out.notes.push(format!("{}: {}", f.id, e.message));
        }
    }
    out
}
