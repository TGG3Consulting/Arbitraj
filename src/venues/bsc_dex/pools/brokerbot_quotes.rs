//! Котировка конкретного объёма сверяется с целочисленными геттерами исходного Brokerbot.
use super::{brokerbot::IBroker, brokerbot_profile::invalid, nami_batch as batch, registry::Entry};
use crate::{core::types::PoolState, gateway::errors::GatewayResult, venues::bsc_dex::ctx::Ctx};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
pub(crate) async fn out_at(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
    at: &Value,
) -> GatewayResult<U256> {
    let PoolState::Brokerbot(s) = &e.pool.state else {
        return Err(invalid(ctx));
    };
    let f = s.swap(input, true, amount).ok_or_else(|| invalid(ctx))?;
    let bought = input == s.cash;
    if bought {
        let get = IBroker::getSharesCall { amount };
        let cost = IBroker::getBuyPriceCall {
            shares: f.amount_out,
        };
        let [units, paid] = batch::read(
            ctx,
            [
                batch::view(at, s.broker, get.clone()),
                batch::view(at, s.broker, cost.clone()),
            ],
        )
        .await?;
        if batch::decode(&units, get)?.min(s.share_balance) != f.amount_out
            || batch::decode(&paid, cost)? != f.amount_in
        {
            return Err(invalid(ctx));
        }
    } else {
        let quote = IBroker::getSellPriceCall { shares: amount };
        let [word] = batch::read(ctx, [batch::view(at, s.broker, quote.clone())]).await?;
        let gross = batch::decode(&word, quote)?;
        let fee = if s.license_bps == 0 {
            U256::ZERO
        } else {
            let fee = IBroker::getLicenseFeeCall { amount: gross };
            let [word] = batch::read(ctx, [batch::view(at, s.broker, fee.clone())]).await?;
            batch::decode(&word, fee)?
        };
        if gross.checked_sub(fee) != Some(f.amount_out) || gross > s.cash_balance {
            return Err(invalid(ctx));
        }
    }
    Ok(f.amount_out)
}

pub(crate) fn request(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    budget: U256,
) -> GatewayResult<(Address, Bytes)> {
    let PoolState::Brokerbot(s) = &e.pool.state else {
        return Err(invalid(ctx));
    };
    // The sell getter remains observable even if the broker currently cannot pay. The
    // associated cash swap in this same simulation then gives the directional refusal.
    if input != s.cash && input != s.share || budget.is_zero() { return Err(invalid(ctx)); }
    let data = if input == s.cash {
        IBroker::getSharesCall { amount: budget }.abi_encode()
    } else {
        IBroker::getSellPriceCall { shares: budget }.abi_encode()
    };
    Ok((s.broker, data.into()))
}
pub(crate) fn payout(e: &Entry, input: Address, budget: U256, data: &[u8]) -> Option<U256> {
    let PoolState::Brokerbot(s) = &e.pool.state else {
        return None;
    };
    let f = s.swap(input, true, budget)?;
    if data.len() != 32 {
        return None;
    }
    let raw = U256::from_be_slice(data);
    let net = if input == s.cash {
        raw.min(s.share_balance)
    } else {
        raw.checked_sub(s.license(raw)?)?
    };
    (net == f.amount_out).then_some(net)
}
