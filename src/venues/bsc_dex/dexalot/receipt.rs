use super::{
    fail,
    profile::Profile,
    wire::{IRfq, IWallet},
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        exec::{self, fill, signed_call::Erc20Call},
        quote::white_state::Head,
        send::receipt::Receipt,
        wallet,
    },
};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::venues::bsc_dex) struct Saved {
    pub call: Erc20Call,
    pub(super) profile: Profile,
    pub(super) nonce: U256,
    pub(super) digest: B256,
    pub(super) expiry: u64,
    pub(super) input: Address,
    pub(super) output: Address,
    pub(super) source: Address,
    pub(super) destination: Address,
    pub(super) taker: Address,
    pub(super) payee: Address,
    pub(super) amount: U256,
    pub(super) quoted: U256,
    pub(super) minimum: U256,
    pub(super) partial: bool,
}
fn unique<E: SolEvent>(logs: &[Log], addr: Address) -> Option<E> {
    let mut it = logs
        .iter()
        .filter(|l| l.address() == addr && l.topic0() == Some(&E::SIGNATURE_HASH));
    let ev = E::decode_log_data(&it.next()?.inner.data).ok()?;
    it.next().is_none().then_some(ev)
}
impl Saved {
    pub(in crate::venues::bsc_dex) fn wallet(&self, c: &Ctx) -> bool {
        c.chain_id() == Some(self.call.chain)
            && wallet::signer::address(c).ok() == Some(self.call.payer)
    }
    pub(in crate::venues::bsc_dex) fn amounts(&self, logs: &[Log]) -> Option<(U256, U256)> {
        let who = self.call.payer;
        let maker = self.profile.maker.addr;
        let e = unique::<IRfq::SwapExecuted>(logs, maker)?;
        if (
            e.nonceAndMeta,
            e.taker,
            e.destTrader,
            u64::from(e.destChainId),
            e.srcAsset,
            e.destAsset,
        ) != (
            self.nonce,
            self.taker,
            self.payee.into_word(),
            self.call.chain,
            self.source,
            self.destination.into_word(),
        ) || e.srcAmount.is_zero()
            || e.srcAmount > self.amount
            || e.destAmount.is_zero()
            || (!self.partial && e.srcAmount != self.amount)
        {
            return None;
        }
        let moved = |a, from, to| fill::moved_logs(logs, a, from, to);
        let spent =
            moved(self.input, Some(who), None).checked_sub(moved(self.input, None, Some(who)))?;
        let net =
            moved(self.output, None, Some(who)).checked_sub(moved(self.output, Some(who), None))?;
        if spent.is_zero() || spent > self.amount || net < self.minimum {
            return None;
        }
        if let Some(m) = self.profile.module {
            let pay = unique::<IWallet::Filled>(logs, m.addr)?;
            if (
                pay.payer,
                pay.venue,
                pay.nonce,
                pay.input,
                pay.output,
                pay.spent,
                pay.received,
            ) != (who, maker, self.nonce, self.input, self.output, spent, net)
                || moved(self.input, Some(who), Some(m.addr)).is_zero()
            {
                return None;
            }
            let source = if self.payee == m.addr {
                m.addr
            } else if self.payee == who {
                maker
            } else {
                return None;
            };
            if moved(self.output, Some(source), Some(who)).is_zero() {
                return None;
            }
        } else if self.payee != who
            || self.taker != who
            || e.srcAmount != spent
            || e.destAmount != net
            || e.destAmount != self.quoted
            || moved(self.input, Some(who), Some(maker)) != spent
            || moved(self.output, Some(maker), Some(who)) != net
        {
            return None;
        }
        Some((spent, net))
    }
    pub(super) async fn measure(
        &self,
        c: &Ctx,
        h: Head,
        data: &Bytes,
    ) -> GatewayResult<(U256, U256, u64)> {
        let (debit, net, gas, logs) = exec::erc20_measure::run(
            c,
            h,
            self.expiry,
            &self.call,
            (self.input, self.output),
            data,
        )
        .await?;
        if self.amounts(&logs) != Some((debit, net)) {
            return Err(fail(
                c,
                "Dexalot: исходный maker и оплаченные суммы не подтверждены",
            ));
        }
        Ok((debit, net, gas))
    }
    pub(in crate::venues::bsc_dex) async fn included(
        &self,
        c: &Ctx,
        tx: B256,
        nonce: u64,
        r: &Receipt,
    ) -> GatewayResult<()> {
        let bad = || {
            c.err(
                ErrorCode::OutcomeUnknown,
                "Dexalot: нет канонического подтверждения исходного поручения и сумм",
            )
        };
        self.call.included(c, tx, nonce, r).await?;
        self.profile
            .maker
            .included(c, tx, r)
            .await
            .map_err(|_| bad())?;
        if let Some(m) = self.profile.module {
            tokio::try_join!(m.included(c, tx, r), self.profile.router.included(c, tx, r))
                .map_err(|_| bad())?;
        }
        if r.status.to::<u64>() == 1 && self.amounts(&r.logs).is_none() {
            return Err(bad());
        }
        Ok(())
    }
}
