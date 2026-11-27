//! Проверка источника акции, валюты и реального кассового остатка после покупки.
use super::{
    probe_calls,
    simulate::{CallResult, SimLog},
};
use crate::{
    core::types::{BrokerbotState, PoolState},
    venues::bsc_dex::pools::registry::Entry,
};
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, U256};
alloy::sol! {
    event Trade(address indexed token,address account,bytes referenceData,int256 shares,address currency,uint256 amount,uint256 fee,uint256 price);
}
pub(super) fn traded(s: &BrokerbotState, log: &SimLog) -> bool {
    log.address == s.broker
        && Trade::decode_raw_log(log.topics.iter().copied(), &log.data).is_ok_and(|ev| {
            ev.token == s.share
                && ev.currency == s.cash
                && !ev.shares.is_zero()
                && !ev.amount.is_zero()
        })
}
pub(super) fn after(
    e: &Entry,
    source: Address,
    amount: U256,
    received: U256,
    balance: Option<&CallResult>,
) -> Option<BrokerbotState> {
    let PoolState::Brokerbot(s) = &e.pool.state else {
        return None;
    };
    let fill = s.swap(source, true, amount)?;
    if fill.amount_out != received {
        return None;
    }
    let mut next = s.after(source, amount)?;
    next.cash_balance = probe_calls::u256(balance.filter(|r| r.ok())?)?;
    Some(next)
}

pub(super) fn one_paid(m: &super::probe::Measured) -> bool {
    m.buy_expected == U256::ONE
        && m.buy_got == U256::ONE
        && m.transfer_sent == U256::ONE
        && m.transfer_got == U256::ONE
        && matches!(&m.sell,Ok((expected,net)) if !expected.is_zero() && expected==net)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_share_requires_the_exact_sale_and_transfer() {
        let mut m = super::super::probe::Measured {
            buy_expected: U256::ONE,
            buy_got: U256::ONE,
            sell: Ok((U256::from(99), U256::from(99))),
            transfer_sent: U256::ONE,
            transfer_got: U256::ONE,
            gas_buy: 1,
            gas_approve_buy: 1,
            gas_transfer: 1,
            gas_sell: None,
        };
        assert!(one_paid(&m));
        m.sell = Ok((U256::from(99), U256::from(98)));
        assert!(!one_paid(&m));
        m.sell = Ok((U256::from(99), U256::from(99)));
        m.transfer_got = U256::ZERO;
        assert!(!one_paid(&m));
    }
}
