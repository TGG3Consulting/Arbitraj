use super::{
    no,
    profile::Profile,
    wire::{IV4, IV5, IWallet, Version},
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        exec::{self, fill, signed_call::Erc20Call},
        pools::solidly_classic_factory::plain,
        quote::white_state::Head,
        send::receipt::Receipt,
        wallet,
    },
};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::venues::bsc_dex) struct Saved {
    pub call: Erc20Call,
    pub(super) profile: Profile,
    pub(super) maker: Address,
    pub(super) nonce: U256,
    pub(super) digest: B256,
    pub(super) expiry: u64,
    pub(super) input: Address,
    pub(super) output: Address,
    pub(super) amount: U256,
    pub(super) quoted: U256,
    pub(super) fee: U256,
    pub(super) minimum: U256,
}
fn unique<E: SolEvent>(logs: &[Log], address: Address) -> Option<E> {
    let mut rows = logs
        .iter()
        .filter(|l| l.address() == address && l.topics().first() == Some(&E::SIGNATURE_HASH));
    let event = E::decode_log_data(&rows.next()?.inner.data).ok()?;
    rows.next().is_none().then_some(event)
}
impl Saved {
    pub(in crate::venues::bsc_dex) fn wallet(&self, ctx: &Ctx) -> bool {
        ctx.chain_id() == Some(self.call.chain)
            && wallet::signer::address(ctx).ok() == Some(self.call.payer)
    }
    pub(in crate::venues::bsc_dex) fn amounts(&self, logs: &[Log]) -> Option<(U256, U256)> {
        let owner = self.call.payer;
        let swap = self.profile.exchange.address;
        let sender = self.profile.sender(owner);
        match self.profile.version {
            Version::V4 => {
                let e = unique::<IV4::SwapERC20>(logs, swap)?;
                if (
                    e.nonce,
                    e.signerWallet,
                    e.signerToken,
                    e.signerAmount,
                    e.protocolFee,
                    e.senderWallet,
                    e.senderToken,
                    e.senderAmount,
                ) != (
                    self.nonce,
                    self.maker,
                    self.output,
                    self.quoted,
                    self.fee,
                    sender,
                    self.input,
                    self.amount,
                ) {
                    return None;
                }
            }
            Version::V5 => {
                let e = unique::<IV5::SwapERC20>(logs, swap)?;
                if (e.nonce, e.signerWallet) != (self.nonce, self.maker) {
                    return None;
                }
            }
        }
        let moved = |t, from, to| fill::moved_logs(logs, t, from, to);
        let paid = moved(self.input, Some(owner), None).checked_sub(moved(
            self.input,
            None,
            Some(owner),
        ))?;
        let net = moved(self.output, None, Some(owner)).checked_sub(moved(
            self.output,
            Some(owner),
            None,
        ))?;
        if self.maker == owner
            || self.maker == sender
            || net < self.minimum
            || paid.is_zero()
            || paid > self.amount
        {
            return None;
        }
        if let Some(module) = self.profile.module {
            let e = unique::<IWallet::Paid>(logs, module.address)?;
            if (
                e.payer, e.maker, e.nonce, e.input, e.output, e.spent, e.received,
            ) != (
                owner,
                self.maker,
                self.nonce,
                self.input,
                self.output,
                paid,
                net,
            ) {
                return None;
            }
        } else if paid != self.amount
            || moved(self.input, Some(owner), Some(self.maker)) != paid
            || moved(self.output, Some(self.maker), Some(owner)) != net
            || net < self.quoted
        {
            return None;
        }
        Some((paid, net))
    }
    pub(super) async fn measure(
        &self,
        ctx: &Ctx,
        h: Head,
        data: &Bytes,
    ) -> GatewayResult<(U256, U256, u64)> {
        let (paid, net, gas, logs) = exec::erc20_measure::run(
            ctx,
            h,
            self.expiry,
            &self.call,
            (self.input, self.output),
            data,
        )
        .await?;
        if self.amounts(&logs) != Some((paid, net)) {
            return Err(no(
                ctx,
                "AirSwap: событие оплаты расходится с остатками владельца",
            ));
        }
        Ok((paid, net, gas))
    }
    pub(in crate::venues::bsc_dex) async fn included(
        &self,
        ctx: &Ctx,
        hash: B256,
        nonce: u64,
        r: &Receipt,
    ) -> GatewayResult<()> {
        self.call.included(ctx, hash, nonce, r).await?;
        let bad = || {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "AirSwap: исходные код или денежный результат не подтверждены",
            )
        };
        let tag = json!({"blockHash":r.block_hash.ok_or_else(bad)?,"requireCanonical":true});
        plain(
            ctx,
            self.profile.exchange.address,
            self.profile.exchange.hash,
            &tag,
        )
        .await
        .map_err(|_| bad())?;
        if let Some(m) = self.profile.module {
            plain(ctx, m.address, m.hash, &tag)
                .await
                .map_err(|_| bad())?;
        }
        if r.status.to::<u64>() == 1 && self.amounts(&r.logs).is_none() {
            return Err(bad());
        }
        Ok(())
    }
}
