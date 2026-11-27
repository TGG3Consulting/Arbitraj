//! Котировки RouterStatic по точному входу в исходных единицах PT, SY или разрешённого ERC-20.
use super::{
    families::Family,
    ladder_counter, load_metric,
    pendle::{self, Series},
    registry::{Entry, Registry},
};
use crate::{
    core::types::{LadderPoint, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::rpc, quote::white_state::Head, tokens::Tokens},
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::json;
sol! {interface Q{
function swapExactPtForSyStatic(address market,uint256 exactPtIn) external view returns(uint256,uint256,uint256,uint256);
function swapExactSyForPtStatic(address market,uint256 exactSyIn) external view returns(uint256,uint256,uint256,uint256);
function swapExactPtForTokenStatic(address market,uint256 exactPtIn,address tokenOut) external view returns(uint256,uint256,uint256,uint256,uint256);
function swapExactTokenForPtStatic(address market,address tokenIn,uint256 amountTokenIn) external view returns(uint256,uint256,uint256,uint256,uint256);
}}
fn data(s: Series, term: Address, buy: bool, n: U256) -> Bytes {
    let market = s.pool;
    match (term == s.sy, buy) {
        (true, true) => Q::swapExactSyForPtStaticCall {
            market,
            exactSyIn: n,
        }
        .abi_encode(),
        (true, false) => Q::swapExactPtForSyStaticCall {
            market,
            exactPtIn: n,
        }
        .abi_encode(),
        (false, true) => Q::swapExactTokenForPtStaticCall {
            market,
            tokenIn: term,
            amountTokenIn: n,
        }
        .abi_encode(),
        (false, false) => Q::swapExactPtForTokenStaticCall {
            market,
            exactPtIn: n,
            tokenOut: term,
        }
        .abi_encode(),
    }
    .into()
}
pub(crate) fn request(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    n: U256,
) -> GatewayResult<(Address, Bytes)> {
    let s = pendle::series(ctx, e.family.id, e.pool.reference.address())?;
    let term = s
        .terminal((e.pool.token0, e.pool.token1))
        .ok_or_else(|| pendle::bad(ctx))?;
    if input != s.pt && input != term {
        return Err(pendle::bad(ctx));
    }
    Ok((
        ctx.dex_addr("dex.quoter", e.family.id)?,
        data(s, term, input == term, n),
    ))
}
pub(crate) fn payout(raw: &[u8]) -> Option<U256> {
    if !matches!(raw.len(), 128 | 160) {
        return None;
    }
    let n = U256::from_be_slice(&raw[..32]);
    (!n.is_zero()).then_some(n)
}
struct Market {
    series: Series,
    term: Address,
    quoter: Address,
    head: Head,
    allows: (bool, bool),
}
impl Market {
    async fn pass(
        &self,
        ctx: &Ctx,
        input: Address,
        sizes: &[U256],
    ) -> GatewayResult<Vec<LadderPoint>> {
        let buy = input == self.term;
        if sizes.is_empty() || !(if buy { self.allows.0 } else { self.allows.1 }) {
            return Ok(Vec::new());
        }
        let at = self.head.selector();
        let req = sizes
            .iter()
            .map(|n| {
                (
                    "eth_call",
                    json!([{"to":self.quoter,"data":data(self.series,self.term,buy,*n)},at]),
                )
            })
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        for (&amount_in, r) in sizes.iter().zip(rpc::batch(ctx, &req).await?) {
            let raw = match r {
                Ok(v) => v,
                Err(e) if e.code == ErrorCode::SwapReverted => continue,
                Err(e) => return Err(e),
            };
            let bytes: Bytes = serde_json::from_str(raw.get())
                .map_err(|e| ctx.err(ErrorCode::BadResponse, e.to_string()))?;
            let words = if self.term == self.series.sy { 4 } else { 5 };
            if bytes.len() != 32 * words {
                return Err(pendle::bad(ctx));
            }
            if let Some(amount_out) = payout(&bytes) {
                out.push(LadderPoint {
                    amount_in,
                    amount_out,
                })
            }
        }
        Ok(out)
    }
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
    h: Head,
) -> GatewayResult<PoolState> {
    let series = pendle::series(ctx, f.id, pool)?;
    let term = series.terminal(pair).ok_or_else(|| pendle::bad(ctx))?;
    let allows = pendle::available(ctx, f, series, term, h).await?;
    let m = Market {
        series,
        term,
        quoter: ctx.dex_addr("dex.quoter", f.id)?,
        head: h,
        allows,
    };
    let coin = if pair.0 == counter { pair.1 } else { pair.0 };
    let (buys, mut sells) = tokio::try_join!(
        ladder_counter::amounts(ctx, tokens, reg, f.id, counter),
        ladder_counter::amounts(ctx, tokens, reg, f.id, coin)
    )?;
    let bought = m.pass(ctx, counter, &buys).await?;
    sells.extend(bought.iter().map(|p| p.amount_out));
    sells.sort_unstable();
    sells.dedup();
    let sold = m.pass(ctx, coin, &sells).await?;
    Ok(load_metric::state(
        pair,
        coin,
        pair.0,
        (!bought.is_empty() || !sold.is_empty()).then_some(0),
        bought,
        sold,
    ))
}
