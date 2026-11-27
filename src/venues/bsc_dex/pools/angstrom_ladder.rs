//! Котировки Angstrom с публичной аттестацией дочернего блока. Каждое чтение
//! проходит исходный V4 Quoter с тем же хуком и данными, что и денежный вызов.
use crate::{
    core::types::LadderPoint,
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        check::simulate::{self, Base, Call},
        ctx::Ctx,
        node::{abi_dex::IV4Quoter, rpc},
        quote::white_state::Head,
    },
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};

pub(super) async fn witness(
    ctx: &Ctx,
    id: &'static str,
    at: &Value,
) -> GatewayResult<Option<(Head, Bytes)>> {
    let h: Head = rpc::call(ctx, "eth_getBlockByHash", json!([at["blockHash"], false])).await?;
    if h.selector() != *at {
        return Err(super::launch_keys::invalid(ctx));
    }
    Ok(super::angstrom_feed::for_child(ctx, id, h.number.to::<u64>()).map(|data| (h, data)))
}
pub(super) async fn quotes(
    ctx: &Ctx,
    viewer: Address,
    h: Head,
    qs: &[(U256, IV4Quoter::quoteExactInputSingleCall)],
) -> GatewayResult<Vec<LadderPoint>> {
    let calls: Vec<_> = qs
        .iter()
        .map(|(_, c)| Call {
            from: Address::ZERO,
            to: viewer,
            input: c.abi_encode().into(),
        })
        .collect();
    let base = Base::for_chain(ctx.chain_id(), h.number.to(), h.timestamp.to());
    let results = simulate::run_anchored(ctx, &calls, &[], &[], base, Some(h.hash)).await?;
    let mut points = Vec::new();
    for ((size, c), r) in qs.iter().zip(results) {
        if !r.ok() {
            continue;
        }
        let out = super::nami_batch::decode(&r.return_data, c.clone())?.amountOut;
        if !out.is_zero() {
            points.push(LadderPoint {
                amount_in: *size,
                amount_out: out,
            });
        }
    }
    Ok(points)
}
