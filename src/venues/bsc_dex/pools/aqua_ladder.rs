//! Входы измеряются независимо в обе стороны. SwapVM должен видеть фактический origin и taker.
use super::{
    aqua::{self, IAqua, Roles},
    aqua_book::Offer,
    aqua_directory, direct_module,
    families::Family,
    ladder_counter,
    load::Loaded,
    registry::Registry,
};
use crate::{
    core::types::{LadderPoint, PoolRef},
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        ctx::Ctx, exec::aqua_wallet::IGuard, node::rpc, tokens::Tokens, wallet::signer,
    },
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};

pub(in crate::venues::bsc_dex) async fn module(
    ctx: &Ctx,
    f: &Family,
    r: Roles,
    at: &Value,
) -> GatewayResult<Address> {
    let addr = ctx.dex_addr("dex.router", f.id)?;
    direct_module::verify_code(ctx, f.id, addr, at).await?;
    let [vm, aqua] = super::nami_batch::read(
        ctx,
        [
            super::nami_batch::view(at, addr, IGuard::engineCall {}),
            super::nami_batch::view(at, addr, IGuard::registryCall {}),
        ],
    )
    .await?;
    if super::nami_batch::decode(&vm, IGuard::engineCall {})? != r.engine
        || super::nami_batch::decode(&aqua, IGuard::registryCall {})? != r.registry
    {
        return Err(aqua::bad(ctx));
    }
    Ok(addr)
}
struct Market<'a> {
    roles: Roles,
    offer: &'a Offer,
    payer: Address,
    module: Option<Address>,
    at: &'a Value,
}
impl Market<'_> {
    async fn levels(
        &self,
        ctx: &Ctx,
        src: Address,
        dst: Address,
        sizes: &[U256],
    ) -> GatewayResult<Vec<LadderPoint>> {
        if sizes.is_empty() {
            return Ok(vec![]);
        }
        let mut reqs = Vec::new();
        let first = self.offer.first;
        // Срок реальной сделки выбирается заново перед подписью.
        let deadline = (1u64 << 40) - 1;
        let taker =
            aqua::traits(U256::ONE, self.payer, deadline, first).ok_or_else(|| aqua::bad(ctx))?;
        let mut jobs = Vec::new();
        for &amount in sizes {
            let call = IAqua::quoteCall {
                order: self.offer.program.clone(),
                tokenIn: src,
                tokenOut: dst,
                amount,
                taker: taker.clone(),
            };
            jobs.push((amount, false));
            reqs.push(("eth_call",json!([{"from":self.payer,"to":self.roles.engine,"data":Bytes::from(call.abi_encode())},self.at])));
            if let Some(module) = self.module {
                let terms = IGuard::Limits {
                    input: src,
                    output: dst,
                    amount,
                    minimum: U256::ONE,
                    deadline: U256::from(deadline),
                    inputFirst: first,
                };
                let call = IGuard::previewCall {
                    order: self.offer.program.clone(),
                    terms,
                    recipient: self.payer,
                };
                jobs.push((amount, true));
                reqs.push(("eth_call",json!([{"from":self.payer,"to":module,"data":Bytes::from(call.abi_encode())},self.at])));
            }
        }
        let results = rpc::batch(ctx, &reqs).await?;
        if results.len() != jobs.len() {
            return Err(aqua::bad(ctx));
        }
        let mut points = std::collections::BTreeMap::<U256, U256>::new();
        for ((size, guarded), r) in jobs.into_iter().zip(results) {
            let Ok(r) = r else { continue };
            let raw: Bytes = serde_json::from_str(r.get()).map_err(|_| aqua::bad(ctx))?;
            let net = if guarded {
                IGuard::previewCall::abi_decode_returns_validate(&raw)
                    .map_err(|_| aqua::bad(ctx))?
            } else {
                let q = IAqua::quoteCall::abi_decode_returns_validate(&raw)
                    .map_err(|_| aqua::bad(ctx))?;
                if q.h != self.offer.hash || q.paid != size {
                    continue;
                }
                q.net
            };
            if !net.is_zero() {
                points
                    .entry(size)
                    .and_modify(|old| *old = (*old).max(net))
                    .or_insert(net);
            }
        }
        Ok(points
            .into_iter()
            .map(|(amount_in, amount_out)| LadderPoint {
                amount_in,
                amount_out,
            })
            .collect())
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    r: &PoolRef,
    pair: (Address, Address),
    cash: Address,
    block: u64,
) -> GatewayResult<Loaded> {
    let generation = ctx.settings.generation();
    let at = super::nami::block(ctx, &format!("{block:#x}")).await?;
    let (roles, offer) = aqua_directory::checked(ctx, f, r, pair, &at).await?;
    let payer = signer::address(ctx)?;
    let market = Market {
        roles,
        offer: &offer,
        payer,
        module: module(ctx, f, roles, &at).await.ok(),
        at: &at,
    };
    let coin = if cash == pair.0 {
        pair.1
    } else if cash == pair.1 {
        pair.0
    } else {
        return Err(aqua::bad(ctx));
    };
    let (buys, mut sells) = tokio::try_join!(
        ladder_counter::amounts(ctx, tokens, reg, f.id, cash),
        ladder_counter::amounts(ctx, tokens, reg, f.id, coin)
    )?;
    let bought = market.levels(ctx, cash, coin, &buys).await?;
    sells.extend(bought.iter().map(|p| p.amount_out));
    sells.sort_unstable();
    sells.dedup();
    let sold = market.levels(ctx, coin, cash, &sells).await?;
    let feasible = (!bought.is_empty() || !sold.is_empty()).then_some(0);
    if generation != ctx.settings.generation() {
        return Err(aqua::bad(ctx));
    }
    Ok(Loaded {
        state: super::load_metric::state(pair, coin, pair.0, feasible, bought, sold),
        ticks: None,
        bins: None,
        fee_pips: None,
    })
}
