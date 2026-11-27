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
