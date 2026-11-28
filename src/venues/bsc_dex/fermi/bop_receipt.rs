//! Источник платежа bopAMM, фактический расход и приход ERC20 по сохранённой квитанции.
//! Для обмена через USDC сверяются обе внутренние суммы и Swap единственного пула USDC/USDT.
use super::{
    bop_cash::{CashFilled, MARKET, POOL, USDC, USDT, WETH},
    slot::{Filled, Proof, Swapped},
};
use crate::venues::bsc_dex::{exec, send};
use alloy::primitives::{Address, B256, I256, U256, keccak256};
use alloy::sol_types::{SolEvent, SolValue};

alloy::sol! {event Swap(address indexed sender,address indexed recipient,int256 amount0,int256 amount1,uint160 sqrtPriceX96,uint128 liquidity,int24 tick);}
const UNI: Address = alloy::primitives::address!("e592427a0aece92de3edee1f18e0157c05861564");
pub(super) fn fill(o: &Proof, r: &send::receipt::Receipt, venue: Address) -> Option<(U256, U256)> {
    let q = o.bop?;
    if venue != MARKET {
        return None;
    }
    let unique = |address: Address, topic: B256| {
        let mut iter = r
            .logs
            .iter()
            .filter(|l| l.address() == address && l.topics().first() == Some(&topic));
        let found = iter.next()?;
        iter.next().is_none().then_some(&found.inner.data)
    };
    let m = Filled::decode_log_data(unique(o.executor, Filled::SIGNATURE_HASH)?).ok()?;
    let s = Swapped::decode_log_data(unique(venue, Swapped::SIGNATURE_HASH)?).ok()?;
    let (spent, received) = if q.bridge {
        let cash =
            CashFilled::decode_log_data(unique(o.executor, CashFilled::SIGNATURE_HASH)?).ok()?;
        if (cash.payer, cash.leg, cash.input, cash.output) != (o.payer, o.leg, o.input, o.output)
            || cash.intermediate.is_zero()
        {
            return None;
        }
        let uni = Swap::decode_log_data(unique(POOL, Swap::SIGNATURE_HASH)?).ok()?;
        if (uni.sender, uni.recipient) != (UNI, o.executor) {
            return None;
        }
        let buy = o.input == USDT;
        let (src, dst, paid, out, usdc, usdt) = if buy {
            (
                USDC,
                WETH,
                cash.intermediate,
                cash.received,
                cash.intermediate,
                cash.spent,
            )
        } else {
            (
                WETH,
                USDC,
                cash.spent,
                cash.intermediate,
                cash.intermediate,
                cash.received,
            )
        };
        let usdc = I256::try_from(usdc).ok()?;
        let usdt = I256::try_from(usdt).ok()?;
        let flows = if buy { (-usdc, usdt) } else { (usdc, -usdt) };
        if (uni.amount0, uni.amount1) != flows
            || m.leg != keccak256((o.payer, o.leg).abi_encode())
            || (m.payer, m.input, m.output, m.spent, m.received)
                != (o.executor, src, dst, paid, out)
        {
            return None;
        }
        (cash.spent, cash.received)
    } else {
        if (m.payer, m.leg, m.input, m.output) != (o.payer, o.leg, o.input, o.output) {
            return None;
        }
        (m.spent, m.received)
    };
    if (
        s.sender,
        s.tokenIn,
        s.tokenOut,
        s.recipient,
        s.amountIn,
        s.amountOut,
    ) != (o.executor, m.input, m.output, m.payer, m.spent, m.received)
        || !o.crossed(spent, received)
    {
        return None;
    }
    let debit = |token| exec::fill::moved(r, token, Some(o.payer), None);
    let credit = |token| exec::fill::moved(r, token, None, Some(o.payer));
    (debit(o.input).checked_sub(credit(o.input))? == spent
        && credit(o.output).checked_sub(debit(o.output))? == received
        && (!q.bridge || debit(USDC) == credit(USDC)))
    .then_some((spent, received))
}

#[cfg(test)]
#[path = "bop_cash_tests.rs"]
mod tests;
