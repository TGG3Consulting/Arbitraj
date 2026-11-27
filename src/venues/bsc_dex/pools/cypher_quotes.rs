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
    async fn answers(&self, ctx: &Ctx, points: &[(Address, Address, U256)], at: &Value)
        -> GatewayResult<Vec<GatewayResult<Option<U256>>>> {
        if points.is_empty() { return Ok(Vec::new()); }
        let requests: Vec<_> = points.iter().map(|&(a,b,n)|
            ("eth_call", self.params(a,b,n,at))).collect();
        let results = rpc::batch(ctx, &requests).await?;
        if results.len() != points.len() {
            return Err(ctx.err(ErrorCode::BadResponse, "Algebra: неполный ответ квотера"));
        }
        Ok(results.into_iter().zip(points).map(|(reply, &(_,_,n))| {
            let raw: Bytes = serde_json::from_str(reply?.get())
                .map_err(|e| ctx.err(ErrorCode::BadResponse, e.to_string()))?;
            Ok(self.output(&raw, n))
        }).collect())
    }
    async fn levels(&self,ctx:&Ctx,a:Address,b:Address,sizes:&[U256],at:&Value)
        -> GatewayResult<Vec<LadderPoint>> {
        let points: Vec<_> = sizes.iter().map(|&n| (a,b,n)).collect();
        let mut out = Vec::new();
        for (&n, reply) in sizes.iter().zip(self.answers(ctx, &points, at).await?) {
            match reply {
                Ok(Some(got)) if !got.is_zero() => out.push(LadderPoint{amount_in:n,amount_out:got}),
                Err(error) if error.code != ErrorCode::SwapReverted => return Err(error),
                _ => {}
            }
        }
        Ok(out)
    }
}
/// Каждая точка видит своего реального плательщика и один и тот же снимок пула и плагина.
pub(in crate::venues::bsc_dex) async fn probes(
    ctx:&Ctx,e:&Entry,points:&[(Address,Address,U256)],tag:&str
) -> GatewayResult<Vec<GatewayResult<U256>>> {
    if points.is_empty() { return Ok(Vec::new()); }
    let pair=(e.pool.token0,e.pool.token1);
    if points.iter().any(|&(a,b,_)| (a,b)!=pair && (b,a)!=pair) {
        return Err(ctx.err(ErrorCode::InvalidAddress, "Пара Algebra не содержит запрошенную сторону"));
    }
    let at=super::elfomo::block(ctx,tag).await?;
    let frame=Pricing::new(ctx,e.family,e.pool.reference.address(),pair,&at).await?;
    Ok(frame.answers(ctx,points,&at).await?.into_iter().map(|reply|
        reply.and_then(|value| value.ok_or_else(||
            ctx.err(ErrorCode::BadResponse,"Algebra: неполный расход или неверное представление котировки")))
    ).collect())
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
