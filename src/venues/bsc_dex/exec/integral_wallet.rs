//! Сохранённое поручение Integral SIZE. Отдельный заказ Delay пополняет Relayer, плательщик получает актив немедленно.
use super::{carbon_binding::Code, fill, signed_call};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, quote::v2_wallet_sim, send::track::Receipt},
};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};
#[path = "integral_abi.rs"]
mod abi;
use abi::IModule;
pub(in crate::venues::bsc_dex) use abi::IRelay;
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Terms {
    pub pair: Address,
    pub target: Address,
    pub relay: Address,
    pub delay: Address,
    pub code: Code,
    pub desired: Option<U256>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        o: &v2_wallet_sim::Order,
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
        let Self {
            terms: t,
            payer: recipient,
            input,
            output,
            amount,
            minimum,
            deadline,
        } = *self;
        let dest = t.desired.unwrap_or(U256::ZERO).max(minimum);
        if t.target == t.relay {
            let Ok(deadline) = u32::try_from(deadline) else {
                return Bytes::new();
            };
            let order = IRelay::Order {
                input,
                output,
                source: amount,
                destination: dest,
                wrap: false,
                recipient,
                deadline,
            };
            return if t.desired.is_some() {
                IRelay::buyCall { order }.abi_encode()
            } else {
                IRelay::sellCall { order }.abi_encode()
            }
            .into();
        }
        if t.desired.is_some() {
            IModule::buyCall {
                input,
                output,
                maxInput: amount,
                targetOut: dest,
                recipient,
                deadline,
            }
            .abi_encode()
        } else {
            IModule::swapFromCall {
                input,
                output,
                amount,
                minimum,
                recipient,
                deadline,
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
            .at_transaction(ctx, self.terms.relay, hash, r, None)
            .await
    }
    pub(in crate::venues::bsc_dex) fn measured(
        &self,
        raw: &[super::super::check::simulate::SimLog],
    ) -> Option<(U256, U256)> {
        self.amounts(
            &signed_call::logs(raw)?,
            self.payer,
            self.input,
            self.output,
        )
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        let t = self.terms;
        if (wallet, input, output) != (self.payer, self.input, self.output)
            || wallet == t.relay
            || wallet == t.delay
            || wallet == t.target
            || input == output
        {
            return None;
        }
        let protected = t.target != t.relay;
        let sender = if protected { t.target } else { wallet };
        let mut events = logs.iter().filter(|l| {
            l.address() == t.relay
                && (l.topic0() == Some(&IRelay::Buy::SIGNATURE_HASH)
                    || l.topic0() == Some(&IRelay::Sell::SIGNATURE_HASH))
        });
        let row = &events.next()?.inner.data;
        let (from, to, a, b, sold, bought, wrap, queue, id, limit) = if t.desired.is_some() {
            let e = IRelay::Buy::decode_log_data(row).ok()?;
            (
                e.sender,
                e.to,
                e.tokenIn,
                e.tokenOut,
                e.amountIn,
                e.amountOut,
                e.wrapUnwrap,
                e.orderContract,
                e.orderId,
                e.amountInMax,
            )
        } else {
            let e = IRelay::Sell::decode_log_data(row).ok()?;
            (
                e.sender,
                e.to,
                e.tokenIn,
                e.tokenOut,
                e.amountIn,
                e.amountOut,
                e.wrapUnwrap,
                e.orderContract,
                e.orderId,
                e.amountOutMin,
            )
        };
        if events.next().is_some()
            || (from, to, a, b, wrap, queue) != (sender, wallet, input, output, false, t.delay)
            || id.is_zero()
            || sold.is_zero()
            || bought.is_zero()
        {
            return None;
        }
        let moved = |a, s, d| fill::moved_logs(logs, a, s, d);
        let debit =
            moved(input, Some(wallet), None).checked_sub(moved(input, None, Some(wallet)))?;
        let credit =
            moved(output, None, Some(wallet)).checked_sub(moved(output, Some(wallet), None))?;
        if debit.is_zero()
            || debit > self.amount
            || credit.is_zero()
            || credit < t.desired.unwrap_or(U256::ZERO).max(self.minimum)
        {
            return None;
        }
        let buy = t.desired.is_some();
        if buy {
            if sold > limit
                || limit > self.amount
                || bought != t.desired?.max(self.minimum)
                || (!protected && limit != self.amount)
            {
                return None;
            }
        } else if debit != self.amount || limit != self.minimum {
            return None;
        }
        if protected {
            let mut f = logs.iter().filter(|e| {
                e.address() == t.target && e.topic0() == Some(&IModule::Filled::SIGNATURE_HASH)
            });
            let e = IModule::Filled::decode_log_data(&f.next()?.inner.data).ok()?;
            if f.next().is_some()
                || (
                    e.payer,
                    e.recipient,
                    e.pair,
                    e.input,
                    e.output,
                    e.spent,
                    e.received,
                    e.orderId,
                    e.exactOutput,
                ) != (
                    wallet, wallet, t.pair, input, output, debit, credit, id, buy,
                )
            {
                return None;
            }
        } else if (sold, bought) != (debit, credit)
            || moved(input, Some(wallet), Some(t.delay)) != debit
            || moved(output, Some(t.relay), Some(wallet)) != credit
        {
            return None;
        }
        Some((debit, credit))
    }
}

#[cfg(test)]
#[path = "integral_wallet_tests.rs"]
mod tests;
