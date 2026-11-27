//! Восстановление Carbon по подписанному распределению, одному итогу контроллера и переводам каждого физического ERC20.
use super::super::{
    check::simulate::SimLog,
    ctx::Ctx,
    pools::carbon::{ICarbon, NATIVE},
    send::track::Receipt,
};
use super::{carbon_wire::Call, fill, signed_call};
use crate::gateway::errors::GatewayResult;
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
    pub call: Call,
    pub payer: Address,
}
impl Proof {
    pub(crate) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        let fail = || {
            ctx.err(
                crate::gateway::errors::ErrorCode::OutcomeUnknown,
                "Carbon: отсутствует привязка подписанного контроллера",
            )
        };
        let code = self.call.code.ok_or_else(fail)?;
        signed_call::included(
            ctx,
            hash,
            r,
            self.payer,
            self.call.recipient(),
            self.call.value(),
            self.call.data(),
        )
        .await?;
        code.at_transaction(ctx, self.call.controller, hash, r)
            .await
    }
    pub(in crate::venues::bsc_dex) fn measured(&self, rows: &[SimLog]) -> Option<(U256, U256)> {
        self.amounts(
            &signed_call::logs(rows)?,
            self.payer,
            self.call.input,
            self.call.output,
        )
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        payer: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        let c = &self.call;
        let fixed = U256::from(c.total()?);
        if payer != self.payer
            || payer.is_zero()
            || payer == c.controller
            || (input, output) != (c.input, c.output)
        {
            return None;
        }
        let mut actions = c.actions.iter();
        let mut actual = None;
        for l in logs.iter().filter(|l| l.address() == c.controller) {
            if l.topic0() == Some(&ICarbon::StrategyUpdated::SIGNATURE_HASH) {
                let u = ICarbon::StrategyUpdated::decode_log_data(&l.inner.data).ok()?;
                if actual.is_some()
                    || actions.next()?.id != u.id
                    || u.reason != 1
                    || !((u.token0, u.token1) == (c.source, c.target)
                        || (u.token0, u.token1) == (c.target, c.source))
                {
                    return None;
                }
            } else if l.topic0() == Some(&ICarbon::TokensTraded::SIGNATURE_HASH) {
                if actions.len() != 0 || actual.is_some() {
                    return None;
                }
                let e = ICarbon::TokensTraded::decode_log_data(&l.inner.data).ok()?;
                if (e.trader, e.sourceToken, e.targetToken, e.byTargetAmount)
                    != (c.module.unwrap_or(payer), c.source, c.target, c.by_target)
                    || e.sourceAmount.is_zero()
                    || e.targetAmount.is_zero()
                {
                    return None;
                }
                let fee = U256::from(e.tradingFeeAmount);
                if c.by_target {
                    if e.targetAmount != fixed
                        || e.sourceAmount > U256::from(c.limit)
                        || fee >= e.sourceAmount
                    {
                        return None;
                    }
                } else if e.sourceAmount != fixed
                    || e.targetAmount < U256::from(c.limit)
                    || e.targetAmount.checked_add(fee).is_none()
                {
                    return None;
                }
                actual = Some((e.sourceAmount, e.targetAmount));
            }
        }
        let (spent, received) = actual?;
        let leg = |asset, from, to, amount| {
            if asset == NATIVE {
                return true;
            }
            let m = |f, t| fill::moved_logs(logs, asset, f, t);
            m(Some(from), Some(to)) == amount
                && m(Some(from), None).checked_sub(m(None, Some(from))) == Some(amount)
                && m(None, Some(to)).checked_sub(m(Some(to), None)) == Some(amount)
        };
        if let Some(module) = c.module {
            if module == payer || module == c.controller || module.is_zero() {
                return None;
            }
            use super::carbon_wire::IWallet;
            let mut rows = logs.iter().filter(|l| {
                l.address() == module && l.topic0() == Some(&IWallet::Settled::SIGNATURE_HASH)
            });
            let s = IWallet::Settled::decode_log_data(&rows.next()?.inner.data).ok()?;
            if rows.next().is_some()
                || (s.payer, s.source, s.target, s.spent) != (payer, c.source, c.target, spent)
                || s.received.is_zero()
                || s.received > received
                || if c.by_target {
                    s.received != fixed
                } else {
                    s.received < U256::from(c.limit)
                }
            {
                return None;
            }
            let delta = |asset, a, inc| {
                let incoming = fill::moved_logs(logs, asset, None, Some(a));
                let outgoing = fill::moved_logs(logs, asset, Some(a), None);
                if inc {
                    incoming.checked_sub(outgoing)
                } else {
                    outgoing.checked_sub(incoming)
                }
            };
            let check =
                |asset, a, inc, amount| asset == NATIVE || delta(asset, a, inc) == Some(amount);
            if !check(c.source, payer, false, spent)
                || !check(c.source, c.controller, true, spent)
                || !check(c.source, module, true, U256::ZERO)
                || !check(c.target, c.controller, false, received)
                || !check(c.target, payer, true, s.received)
                || !check(c.target, module, true, U256::ZERO)
            {
                return None;
            }
            return Some((spent, s.received));
        }
        (leg(c.source, payer, c.controller, spent) && leg(c.target, c.controller, payer, received))
            .then_some((spent, received))
    }
}
#[cfg(test)]
#[path = "carbon_fact_tests.rs"]
mod tests;
