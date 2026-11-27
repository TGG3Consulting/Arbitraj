//! Восстановление исходного платежа по подписанному хешу, каждому maker Fill и двум дельтам кошелька.
use super::wire::{IReactor, IWallet, Slice};
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
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::venues::bsc_dex) struct Saved {
    pub call: Erc20Call,
    pub(super) reactor: Address,
    pub(super) collector: Address,
    pub(super) action: B256,
    pub(super) input: Address,
    pub(super) output: Address,
    pub(super) debit: U256,
    pub(super) minimum: U256,
    pub(super) expiry: u64,
    pub(super) slices: Vec<Slice>,
}
impl Saved {
    pub(in crate::venues::bsc_dex) fn wallet(&self, ctx: &Ctx) -> bool {
        ctx.chain_id() == Some(self.call.chain)
            && wallet::signer::address(ctx).ok() == Some(self.call.payer)
    }
    pub(in crate::venues::bsc_dex) fn amounts(&self, logs: &[Log]) -> Option<(U256, U256)> {
        let owner = self.call.payer;
        let module = self.call.module;
        let filler = if module == self.reactor {
            owner
        } else {
            module
        };
        let net = |token| {
            (
                fill::moved_logs(logs, token, Some(owner), None),
                fill::moved_logs(logs, token, None, Some(owner)),
            )
        };
        let (spent, refund) = net(self.input);
        let (sent, credit) = net(self.output);
        let paid = spent.checked_sub(refund)?;
        let received = credit.checked_sub(sent)?;
        if paid != self.debit || received < self.minimum {
            return None;
        }
        let mut matched = HashSet::new();
        for row in logs.iter().filter(|l| {
            l.address() == self.reactor
                && l.topics().first() == Some(&IReactor::Fill::SIGNATURE_HASH)
        }) {
            let x = IReactor::Fill::decode_log_data(&row.inner.data).ok()?;
            if x.filler != filler
                || !self
                    .slices
                    .iter()
                    .any(|s| s.hash == x.orderHash && s.nonce == x.nonce && s.maker == x.swapper)
                || !matched.insert(x.orderHash)
            {
                return None;
            }
        }
        if matched.len() != self.slices.len() {
            return None;
        }
        let fee = self
            .slices
            .iter()
            .try_fold(U256::ZERO, |n, s| n.checked_add(s.fee))?;
        if fill::moved_logs(logs, self.input, Some(filler), Some(self.collector)) != fee {
            return None;
        }
        if module != self.reactor {
            let mut events = logs.iter().filter(|l| {
                l.address() == module
                    && l.topics().first() == Some(&IWallet::Settled::SIGNATURE_HASH)
            });
            let e = IWallet::Settled::decode_log_data(&events.next()?.inner.data).ok()?;
            if events.next().is_some()
                || (
                    e.payer,
                    e.action,
                    e.sellToken,
                    e.buyToken,
                    e.spent,
                    e.received,
                ) != (owner, self.action, self.input, self.output, paid, received)
            {
                return None;
            }
        }
        Some((paid, received))
    }
    pub(super) async fn measure(
        &self,
        ctx: &Ctx,
        h: Head,
        data: &Bytes,
    ) -> GatewayResult<(U256, u64)> {
        let (paid, received, gas, logs) = exec::erc20_measure::run(
            ctx,
            h,
            self.expiry,
            &self.call,
            (self.input, self.output),
            data,
        )
        .await?;
        if self.amounts(&logs) != Some((paid, received)) {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "Textile: денежные дельты не совпадают с подписанными ордерами",
            ));
        }
        Ok((received, gas))
    }
    pub(in crate::venues::bsc_dex) async fn included(
        &self,
        ctx: &Ctx,
        hash: B256,
        nonce: u64,
        r: &Receipt,
    ) -> GatewayResult<()> {
        self.call.included(ctx, hash, nonce, r).await?;
        if r.status.to::<u64>() == 1 && self.amounts(&r.logs).is_none() {
            return Err(ctx.err(
                ErrorCode::OutcomeUnknown,
                "Textile: квитанция не подтвердила исходный расчёт",
            ));
        }
        Ok(())
    }
}
