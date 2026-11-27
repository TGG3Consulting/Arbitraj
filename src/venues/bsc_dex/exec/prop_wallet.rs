//! Сверка PropAMMRouter по исходной подписи, реализации прокси и фактическому рынку выплаты.
use super::{
    carbon_binding::Code,
    fill,
    prop_abi::{IProp, IStrict},
    signed_call,
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        check::simulate::SimLog, ctx::Ctx, node::abi::IERC20, quote::v2_wallet_sim::Order,
        send::track::Receipt,
    },
};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Terms {
    pub target: Address,
    pub fallback_allowed: bool,
    pub router: Address,
    pub venue: Address,
    pub vault: Address,
    pub fallback: Address,
    pub code: Code,
    pub venue_code: Option<Code>,
    pub vault_code: Option<Code>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub terms: Terms,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Terms {
    pub(in crate::venues::bsc_dex) fn proof(
        self,
        o: &Order,
        minimum: U256,
        deadline: U256,
    ) -> Proof {
        Proof {
            terms: self,
            payer: o.payer,
            input: o.input,
            output: o.output,
            amount: o.amount,
            minimum,
            deadline,
        }
    }
}
impl Proof {
    pub(in crate::venues::bsc_dex) fn data(&self) -> Bytes {
        let t = self.terms;
        if t.target == t.router {
            IProp::swapViaVenueV1Call {
                venue: t.venue,
                tokenIn: self.input,
                tokenOut: self.output,
                amountIn: self.amount,
                minOut: self.minimum,
                recipient: self.payer,
                deadline: self.deadline,
            }
            .abi_encode()
        } else if t.fallback_allowed {
            IStrict::executeWithFallbackCall {
                venue: t.venue,
                input: self.input,
                output: self.output,
                amount: self.amount,
                minimum: self.minimum,
                deadline: self.deadline,
                fallbackRouter: t.fallback,
            }
            .abi_encode()
        } else {
            IStrict::executeCall {
                venue: t.venue,
                input: self.input,
                output: self.output,
                amount: self.amount,
                minimum: self.minimum,
                deadline: self.deadline,
            }
            .abi_encode()
        }
        .into()
    }
    pub(super) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            hash,
            r,
            self.payer,
            self.terms.target,
            U256::ZERO,
            self.data(),
        )
        .await?;
        self.terms
            .code
            .at_transaction(ctx, self.terms.router, hash, r, None)
            .await?;
        let source=self.outcome(&r.logs).map(|(who,_,_)|who).ok_or_else(||ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,"Источник выплаты LambdaClass не подтверждён"))?;
        if source==self.terms.fallback{return Ok(())}
        for (code, role) in [
            (self.terms.venue_code, self.terms.venue),
            (self.terms.vault_code, self.terms.vault),
        ] {
            if let Some(c) = code {
                c.at_transaction(ctx, role, hash, r, None).await?
            }
        }
        Ok(())
    }
    pub(in crate::venues::bsc_dex) fn measured(&self, logs: &[SimLog]) -> Option<(U256, U256)> {
        let (_, paid, received) = self.outcome(&signed_call::logs(logs)?)?;
        Some((paid, received))
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        if (self.payer, self.input, self.output) != (wallet, input, output) {
            return None;
        }
        let (_, paid, received) = self.outcome(logs)?;
        Some((paid, received))
    }
    pub(super) fn outcome(&self, logs: &[Log]) -> Option<(Address, U256, U256)> {
        let t = self.terms;
        let protected = t.target != t.router;
        let owner = self.payer;
        if self.input == self.output
            || owner == t.venue
            || owner == t.vault
            || owner == t.router
            || owner == t.fallback
            || owner == t.target
        {
            return None;
        }
        let sender = if protected { t.target } else { owner };
        let mut swaps = logs.iter().filter(|l| {
            l.address() == t.router && l.topic0() == Some(&IProp::Swapped::SIGNATURE_HASH)
        });
        let s = IProp::Swapped::decode_log_data(&swaps.next()?.inner.data).ok()?;
        if swaps.next().is_some()
            || (s.sender, s.recipient, s.tokenIn, s.tokenOut)
                != (sender, owner, self.input, self.output)
            || s.amountOut.is_zero()
            || s.amountOut < self.minimum
            || s.amountIn.is_zero()
        {
            return None;
        }
        let moved = |coin, from, to| fill::moved_logs(logs, coin, from, to);
        let debit = moved(self.input, Some(owner), None).checked_sub(moved(
            self.input,
            None,
            Some(owner),
        ))?;
        let credit = moved(self.output, None, Some(owner)).checked_sub(moved(
            self.output,
            Some(owner),
            None,
        ))?;
        if debit != self.amount || credit != s.amountOut || debit < s.amountIn {
            return None;
        }
        if protected {
            let mut events = logs.iter().filter(|l| {
                l.address() == t.target && l.topic0() == Some(&IStrict::Filled::SIGNATURE_HASH)
            });
            let f = IStrict::Filled::decode_log_data(&events.next()?.inner.data).ok()?;
            if events.next().is_some()
                || (
                    f.payer,
                    f.recipient,
                    f.venue,
                    f.input,
                    f.output,
                    f.spent,
                    f.received,
                ) != (
                    owner,
                    owner,
                    s.marketMaker,
                    self.input,
                    self.output,
                    debit,
                    credit,
                )
                || moved(self.input, Some(owner), Some(t.target)) != s.amountIn
            {
                return None;
            }
        } else if debit != s.amountIn {
            return None;
        }
        if s.marketMaker == t.venue {
            if moved(self.input, Some(sender), Some(t.venue)) != s.amountIn
                || (t.venue != t.vault && moved(self.input, Some(t.venue), Some(t.vault)) != s.amountIn)
                || moved(self.output, Some(t.vault), Some(owner)) != credit
            {
                return None;
            }
        } else if s.marketMaker == t.fallback && t.fallback_allowed {
            if moved(self.input, Some(sender), Some(t.router)) != s.amountIn {
                return None;
            }
            // Пара Uniswap получает вход от прокси и сама отправляет выход получателю.
            let mut pools = logs
                .iter()
                .filter(|l| l.address() == self.output)
                .filter_map(|l| IERC20::Transfer::decode_log_data(&l.inner.data).ok())
                .filter(|tr| tr.to == owner && !tr.value.is_zero());
            let pool = pools.next()?;
            if pools.next().is_some()
                || pool.value != credit
                || moved(self.input, Some(t.router), Some(pool.from)) != s.amountIn
            {
                return None;
            }
        } else {
            return None;
        }
        Some((s.marketMaker, debit, credit))
    }
}

#[cfg(test)]
#[path = "prop_wallet_tests.rs"]
mod tests;
