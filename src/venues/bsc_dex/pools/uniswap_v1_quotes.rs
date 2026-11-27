//! Original V1 get*Price operands. For dual ERC20 quotes both physical exchanges are queried at one canonical hash.
use super::{nami, registry::Entry, uniswap_v1::IExchange};
use crate::{
    core::types::{PoolState, uniswap_v1::UniswapV1State},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{multicall, rpc},
    },
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};

fn state(e: &Entry) -> Option<&UniswapV1State> {
    match &e.pool.state {
        PoolState::UniswapV1(s) => Some(s),
        _ => None,
    }
}
fn no(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Uniswap V1: не подтверждены оба расчёта физических бирж",
    )
}
fn price(token_to_eth: bool, exact_in: bool, amount: U256) -> Bytes {
    match (token_to_eth, exact_in) {
        (true, true) => IExchange::getTokenToEthInputPriceCall { amount }.abi_encode(),
        (false, true) => IExchange::getEthToTokenInputPriceCall { amount }.abi_encode(),
        (true, false) => IExchange::getTokenToEthOutputPriceCall { amount }.abi_encode(),
        (false, false) => IExchange::getEthToTokenOutputPriceCall { amount }.abi_encode(),
    }
    .into()
}
fn number(data: &[u8]) -> Option<U256> {
    matches!(data.len(), 32 | 4096)
        .then(|| U256::from_be_slice(&data[..32]))
        .filter(|x| !x.is_zero())
}
async fn read(ctx: &Ctx, pool: Address, data: Bytes, at: &Value) -> GatewayResult<U256> {
    let raw: Bytes = rpc::call(ctx, "eth_call", json!([{"to":pool,"data":data},at])).await?;
    number(&raw).ok_or_else(|| no(ctx))
}

pub(crate) async fn official(
    ctx: &Ctx,
    s: &UniswapV1State,
    source: Address,
    exact_in: bool,
    amount: U256,
    at: &Value,
) -> GatewayResult<U256> {
    let i = s.side(source).ok_or_else(|| no(ctx))?;
    let a = s.markets[i].as_ref();
    let b = s.markets[1 - i].as_ref();
    let order = if exact_in {
        [(a, true), (b, false)]
    } else {
        [(b, false), (a, true)]
    };
    let mut current = amount;
    for (ex, token_to_eth) in order {
        if let Some(ex) = ex {
            current = read(ctx, ex.address, price(token_to_eth, exact_in, current), at).await?;
        }
    }
    Ok(current)
}
pub(crate) async fn out(
    ctx: &Ctx,
    e: &Entry,
    source: Address,
    amount: U256,
    tag: &str,
) -> GatewayResult<U256> {
    let at = nami::block(ctx, tag).await?;
    official(
        ctx,
        state(e).ok_or_else(|| no(ctx))?,
        source,
        true,
        amount,
        &at,
    )
    .await
}

/// An eth_simulateV1 prefix needs a single request. The first result authenticates the amount passed into the second.
pub(crate) fn request(
    ctx: &Ctx,
    e: &Entry,
    source: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let s = state(e).ok_or_else(|| no(ctx))?;
    let i = s.side(source).ok_or_else(|| no(ctx))?;
    let a = s.markets[i].as_ref();
    let b = s.markets[1 - i].as_ref();
    match (a, b) {
        (Some(a), None) => Ok((a.address, price(true, true, amount))),
        (None, Some(b)) => Ok((b.address, price(false, true, amount))),
        (Some(a), Some(b)) => {
            let mid = s.swap(source, true, amount).ok_or_else(|| no(ctx))?.eth;
            let mc = multicall::address(ctx).ok_or_else(|| {
                ctx.not_set(
                    "dex.multicall3",
                    "Uniswap V1: нужен один вызов к двум физическим биржам",
                )
            })?;
            Ok((
                mc,
                multicall::encode(&[
                    (a.address, price(true, true, amount)),
                    (b.address, price(false, true, mid)),
                ]),
            ))
        }
        _ => Err(no(ctx)),
    }
}
pub(crate) fn decode(e: &Entry, data: &[u8], source: Address, amount: U256) -> Option<U256> {
    let s = state(e)?;
    if !s.dual() {
        return number(data);
    }
    let mut values = multicall::decode(data, 2).ok()?.into_iter();
    let observed = number(&values.next()?.ok()?)?;
    if observed != s.swap(source, true, amount)?.eth {
        return None;
    }
    number(&values.next()?.ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_forwarder_returns_the_first_word_and_trailing_vyper_memory() {
        let mut raw = vec![0xab; 4096];
        raw[..32].copy_from_slice(&U256::from(16).to_be_bytes::<32>());
        assert_eq!(number(&raw), Some(U256::from(16)));
        assert_eq!(number(&raw[..32]), Some(U256::from(16)));
        assert_eq!(number(&raw[..31]), None);
        assert_eq!(number(&raw[1..]), None);
    }
}
