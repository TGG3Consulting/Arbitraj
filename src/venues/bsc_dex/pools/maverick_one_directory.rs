//! Реестр Maverick V1. Фабрика создаёт пулы инструкцией CREATE, регистрацию подтверждает isFactoryPool.
use super::{
    families::{self, Family},
    family_variant::Variant,
    maverick_one::IMaverickOne as M,
    stable_list::Extra,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        directory::gt_parse::IndexedPool,
        node::{multicall, rpc},
    },
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U64};
use serde_json::json;
use std::collections::HashMap;

async fn registry(
    ctx: &Ctx,
    f: &Family,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
) -> GatewayResult<Vec<IndexedPool>> {
    let at = super::nami::block(ctx, "latest").await?;
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let hash = ctx
        .dex_text("dex.factory_runtime_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| {
            ctx.err(
                ErrorCode::NotConfigured,
                "Maverick V1: требуется хеш фабрики",
            )
        })?;
    super::solidly_classic_factory::plain(ctx, factory, hash, &at).await?;
    let count: U64 = rpc::call(ctx, "eth_getTransactionCount", json!([factory, at])).await?;
    let limit = count.to::<u64>();
    let target = ctx.addr("dex.multicall3")?;
    let step = ctx.int("dex.verify_batch")?.max(1);
    let mut found = Vec::new();
    let mut nonce = 1u64;
    while nonce < limit {
        let next = nonce.saturating_add(step).min(limit);
        let mut calls = Vec::new();
        for n in nonce..next {
            let pool = factory.create(n);
            calls.extend([
                (factory, M::isFactoryPoolCall { pool }.abi_encode().into()),
                (pool, M::tokenACall {}.abi_encode().into()),
                (pool, M::tokenBCall {}.abi_encode().into()),
            ]);
        }
        let raw: Bytes = rpc::call(
            ctx,
            "eth_call",
            json!([{"to":target,"data":multicall::encode(&calls)},at]),
        )
        .await?;
        let answers = multicall::decode(&raw, calls.len())?;
        for (n, three) in (nonce..next).zip(answers.chunks_exact(3)) {
            let [Ok(reg), Ok(a), Ok(b)] = three else {
                continue;
            };
            if !matches!(M::isFactoryPoolCall::abi_decode_returns(reg), Ok(true)) {
                continue;
            }
            let (Ok(a), Ok(b)) = (
                M::tokenACall::abi_decode_returns(a),
                M::tokenBCall::abi_decode_returns(b),
            ) else {
                continue;
            };
            for (base, quote) in [(a, b), (b, a)] {
                if let Some(symbol) = coins.get(&base)
                    && quotes.contains(&quote)
                {
                    found.push(IndexedPool {
                        dex_id: f.id.into(),
                        pool: format!("{:#x}", factory.create(n)),
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
        nonce = next;
    }
    Ok(found)
}
pub(super) async fn scan(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut out = Extra::default();
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| f.variant == Variant::MaverickOne)
    {
        match registry(ctx, f, coins, quotes).await {
            Ok(rows) => {
                for row in rows {
                    out.found.entry(row.base).or_default().push(row);
                }
            }
            Err(e) => {
                out.no_answer += 1;
                out.notes.push(format!("{}: {}", f.id, e.message));
            }
        }
    }
    out
}
