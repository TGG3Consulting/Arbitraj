//! Уровни Quoter Cypher рассчитаны независимо по каждой стороне. Каждый запрос привязан к хешу и своему tx.origin.
use super::{
    cypher,
    families::Family,
    ladder_counter, load_metric, nami,
    registry::{Entry, Registry},
};
use crate::{
    core::types::{LadderPoint, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::rpc, tokens::Tokens, wallet::signer},
};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};
struct Pricing {
    payer: Address,
    quoter: Address,
    custom: Address,
    tuple: bool,
}
impl Pricing {
    async fn new(
        ctx: &Ctx,
        f: &Family,
        pool: Address,
        pair: (Address, Address),
        at: &Value,
    ) -> GatewayResult<Self> {
        Ok(Self {
            payer: signer::address(ctx)?,
            tuple: f.variant == super::family_variant::Variant::TheoremIntegral,
            quoter: ctx.dex_addr("dex.quoter", f.id)?,
            custom: cypher::checked(ctx, f, pool, pair, at).await?,
        })
    }
    fn params(&self, a: Address, b: Address, n: U256, at: &Value) -> Value {
        let data = if self.tuple { super::theorem::calldata(a,b,n,self.custom) } else { nami::custom_calldata(a,b,n,self.custom) };
        json!([{"from":self.payer,"to":self.quoter,"data":data},at])
    }
    fn output(&self, raw: &[u8], amount: U256) -> Option<U256> {
        if self.tuple {super::theorem::output(raw,amount)} else {nami::decoded(raw)}
    }
    async fn levels(
        &self,
        ctx: &Ctx,
        a: Address,
        b: Address,
        sizes: &[U256],
        at: &Value,
    ) -> GatewayResult<Vec<LadderPoint>> {
        if sizes.is_empty() {
            return Ok(Vec::new());
        }
        let requests: Vec<_> = sizes
            .iter()
            .map(|&n| ("eth_call", self.params(a, b, n, at)))
            .collect();
        let result = rpc::batch(ctx, &requests).await?;
        if result.len() != sizes.len() {
            return Err(ctx.err(ErrorCode::BadResponse, "Cypher: неполный ответ квотера"));
        }
        let mut out = Vec::new();
        for (&n, r) in sizes.iter().zip(result) {
            let raw = match r {
                Ok(r) => r,
                Err(err) if err.code == ErrorCode::SwapReverted => continue,
                Err(err) => return Err(err),
            };
            let raw: Bytes = serde_json::from_str(raw.get())
                .map_err(|e| ctx.err(ErrorCode::BadResponse, e.to_string()))?;
            if let Some(got) = self.output(&raw,n).filter(|v| !v.is_zero()) {
                out.push(LadderPoint {
                    amount_in: n,
                    amount_out: got,
                })
            }
        }
        Ok(out)
    }
}
pub(in crate::venues::bsc_dex) async fn out(
    ctx: &Ctx,
    e: &Entry,
    a: Address,
    b: Address,
    n: U256,
    tag: &str,
) -> GatewayResult<U256> {
    let at = super::elfomo::block(ctx, tag).await?;
    let frame = Pricing::new(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.pool.token0, e.pool.token1),
        &at,
    )
    .await?;
    let raw: Bytes = rpc::call(ctx, "eth_call", frame.params(a, b, n, &at)).await?;
    frame.output(&raw,n).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            "Cypher: ответ официального квотера не по ABI",
        )
    })
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    counter: Address,
    at: &Value,
) -> GatewayResult<PoolState> {
    let frame = Pricing::new(ctx, f, pool, pair, at).await?;
    let coin = if pair.0 == counter { pair.1 } else { pair.0 };
    let (buy, mut sell) = tokio::try_join!(
        ladder_counter::amounts(ctx, tokens, reg, f.id, counter),
        ladder_counter::amounts(ctx, tokens, reg, f.id, coin)
    )?;
    let bids = frame.levels(ctx, counter, coin, &buy, at).await?;
    sell.extend(bids.iter().map(|r| r.amount_out));
    sell.sort_unstable();
    sell.dedup();
    let offers = frame.levels(ctx, coin, counter, &sell, at).await?;
    Ok(load_metric::state(
        pair,
        coin,
        pair.0,
        (!bids.is_empty() || !offers.is_empty()).then_some(0),
        bids,
        offers,
    ))
}
