//! Прогноз штатного FOT-вызова. Учитывает отдельный adminFee и округления пары. Налог самого токена измеряет проба.
//! DEGENSwapRouter.sol, getAmountsOut, _calculateAmounts, swapExactTokensForTokensSupportingFeeOnTransferTokens.
//! https://etherscan.io/address/0x4bf3e2287d4ced7796bfab364c0401dfce4a4f7f#code
use super::{
    abi_check::IPoolImmutables as Pool, degenswap::IDegen as D, families::Family, registry::Entry,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::multicall},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

pub(super) fn wire(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    input: Address,
    output: Address,
    n: U256,
) -> GatewayResult<(Address, Bytes)> {
    let router = ctx.dex_addr("dex.quoter", f.id)?;
    let first = input < output;
    let t0 = Bytes::from(Pool::token0Call {}.abi_encode());
    let t1 = Bytes::from(Pool::token1Call {}.abi_encode());
    let (src, dst) = if first { (t0, t1) } else { (t1, t0) };
    let balances = Bytes::from(D::balanceOfCall { owner: pool }.abi_encode());
    let calls = [
        (
            router,
            D::getAmountsOutCall {
                amount: n,
                path: vec![input, output],
            }
            .abi_encode()
            .into(),
        ),
        (pool, src),
        (pool, dst),
        (pool, D::getReservesCall {}.abi_encode().into()),
        (pool, D::getTotalFeeCall {}.abi_encode().into()),
        (
            router,
            D::pairFeeAddressCall { pair: pool }.abi_encode().into(),
        ),
        (router, D::adminFeeCall {}.abi_encode().into()),
        (input, balances.clone()),
        (output, balances),
    ];
    Ok((ctx.addr("dex.multicall3")?, multicall::encode(&calls)))
}
pub(in crate::venues::bsc_dex) fn request(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    n: U256,
) -> GatewayResult<(Address, Bytes)> {
    let p = &e.pool;
    let out = if input == p.token0 {
        p.token1
    } else if input == p.token1 {
        p.token0
    } else {
        return Err(ctx.err(ErrorCode::NoRoute, "DEGENSwap: вход отсутствует в паре"));
    };
    wire(ctx, e.family, p.reference.address(), input, out, n)
}
fn less_fee(n: U256, rate: U256) -> Option<U256> {
    n.checked_sub(n.checked_mul(rate)? / U256::from(10000u64))
}
pub(in crate::venues::bsc_dex) fn payout(raw: &[u8]) -> Option<U256> {
    let rows = multicall::decode(raw, 9)
        .ok()?
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let quoted = D::getAmountsOutCall::abi_decode_returns_validate(&rows[0]).ok()?;
    let [n, reference] = quoted.as_slice() else {
        return None;
    };
    let input = Pool::token0Call::abi_decode_returns_validate(&rows[1]).ok()?;
    let output = Pool::token0Call::abi_decode_returns_validate(&rows[2]).ok()?;
    let r = D::getReservesCall::abi_decode_returns_validate(&rows[3]).ok()?;
    let total = D::getTotalFeeCall::abi_decode_returns_validate(&rows[4]).ok()?;
    let fee_asset = D::pairFeeAddressCall::abi_decode_returns_validate(&rows[5]).ok()?;
    let admin = D::adminFeeCall::abi_decode_returns_validate(&rows[6]).ok()?;
    let balance_in = D::balanceOfCall::abi_decode_returns_validate(&rows[7]).ok()?;
    let balance_out = D::balanceOfCall::abi_decode_returns_validate(&rows[8]).ok()?;
    if input == output || input.is_zero() || output.is_zero() || admin > U256::from(100) {
        return None;
    }
    let (rin, rout) = if input < output {
        (r.r0, r.r1)
    } else {
        (r.r1, r.r0)
    };
    let (rin, rout) = (U256::from(rin), U256::from(rout));
    if rin.is_zero() || rout.is_zero() {
        return None;
    }
    let unit = U256::from(10000);
    let rate = if r.base.is_zero() { U256::ZERO } else { total };
    let multiplier = unit.checked_sub(rate)?;
    let gross = |dx: U256| {
        let weighted = dx.checked_mul(multiplier)?;
        weighted
            .checked_mul(rout)?
            .checked_div(rin.checked_mul(unit)?.checked_add(weighted)?)
    };
    if gross(*n)? != *reference {
        return None;
    }
    let own = if fee_asset == input {
        less_fee(*n, admin)?
    } else {
        *n
    };
    // Чужой положительный остаток не входит в план. Дефицит сокращает оплаченный вход.
    let paid = own.checked_sub(rin.saturating_sub(balance_in))?;
    let amount = gross(paid)?;
    if !r.base.is_zero() {
        let raw_fee = if r.base == input { paid } else { amount }.checked_mul(total)? / unit;
        if raw_fee.is_zero() {
            return None;
        }
    }
    if amount > balance_out {
        return None;
    }
    let net = if fee_asset == output {
        less_fee(amount, admin)?
    } else {
        amount
    };
    (!net.is_zero()).then_some(net)
}
