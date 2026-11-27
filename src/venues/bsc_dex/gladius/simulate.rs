//! Сверка самой оплаты кошельком по реальным остаткам и двум разным событиям.
use super::{abi::*, model::Offer, profile::Profile, *};
use crate::venues::bsc_dex::{
    exec::{carbon_binding::Code, signed_call::Erc20Call},
    node::abi::IERC20,
    send::receipt::Receipt,
    wallet,
};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{U64, keccak256};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Saved {
    pub(super) call: Erc20Call,
    profile: Profile,
    digest: B256,
    hash: B256,
    maker: Address,
    nonce: U256,
    input: Address,
    output: Address,
    minimum_debit: U256,
    maximum_debit: U256,
    minimum_credit: U256,
    quantity: U256,
}
impl Saved {
    pub(in crate::venues::bsc_dex) fn wallet(&self, ctx: &Ctx) -> bool {
        ctx.chain_id() == Some(self.call.chain)
            && wallet::signer::address(ctx).ok() == Some(self.call.payer)
    }
    pub(in crate::venues::bsc_dex) async fn included(
        &self,
        ctx: &Ctx,
        hash: B256,
        nonce: u64,
        r: &Receipt,
    ) -> GatewayResult<()> {
        self.call.included(ctx, hash, nonce, r).await?;
        if r.status == U64::from(1) {
            Code {
                shell: self.profile.reactor.hash,
                implementation: self.profile.implementation.address,
                runtime: self.profile.implementation.hash,
            }
            .at_transaction(ctx, self.profile.reactor.address, hash, r, None)
            .await?;
            if self.amounts(&r.logs).is_none() {
                return Err(ctx.err(
                    ErrorCode::OutcomeUnknown,
                    "Gladius: события и движения средств расходятся",
                ));
            }
        }
        Ok(())
    }
    pub(in crate::venues::bsc_dex) fn amounts(&self, logs: &[Log]) -> Option<(U256, U256)> {
        let event = |address, topic| {
            let mut rows = logs
                .iter()
                .filter(|l| l.address() == address && l.topics().first() == Some(&topic));
            let row = rows.next()?;
            rows.next().is_none().then_some(row)
        };
        let filled = IReactor::Fill::decode_log_data(
            &event(self.profile.reactor.address, IReactor::Fill::SIGNATURE_HASH)?
                .inner
                .data,
        )
        .ok()?;
        let paid = IWallet::Settled::decode_log_data(
            &event(self.call.module, IWallet::Settled::SIGNATURE_HASH)?
                .inner
                .data,
        )
        .ok()?;
        if filled.orderHash != self.hash
            || filled.filler != self.call.module
            || filled.swapper != self.maker
            || filled.nonce != self.nonce
            || paid.payer != self.call.payer
            || paid.orderDigest != self.digest
            || paid.input != self.input
            || paid.output != self.output
            || paid.spent < self.minimum_debit
            || paid.spent > self.maximum_debit
            || paid.spent.is_zero()
            || paid.received < self.minimum_credit
        {
            return None;
        }
        let delta = |asset| {
            let (mut debit, mut credit) = (U256::ZERO, U256::ZERO);
            for l in logs.iter().filter(|l| {
                l.address() == asset
                    && l.topics().first() == Some(&IERC20::Transfer::SIGNATURE_HASH)
            }) {
                let t = IERC20::Transfer::decode_log_data(&l.inner.data).ok()?;
                if t.from == self.call.payer {
                    debit = debit.checked_add(t.value)?;
                }
                if t.to == self.call.payer {
                    credit = credit.checked_add(t.value)?;
                }
            }
            Some((debit, credit))
        };
        let maker_debit = crate::venues::bsc_dex::exec::fill::moved_logs(
            logs,
            self.output,
            Some(self.maker),
            None,
        );
        if maker_debit != self.quantity {
            return None;
        }
        let (d, c) = delta(self.input)?;
        let (od, oc) = delta(self.output)?;
        (d.checked_sub(c)? == paid.spent && oc.checked_sub(od)? == paid.received)
            .then_some((paid.spent, paid.received))
    }
}
pub(super) struct Payment {
    pub data: Bytes,
    pub saved: Saved,
}
impl Payment {
    pub(super) fn new(
        profile: Profile,
        owner: Address,
        o: &Offer,
        quantity: U256,
        low: U256,
        cap: U256,
        minimum: U256,
    ) -> Self {
        let (input, output) = o.pair();
        let data = Bytes::from(
            IWallet::fillCall {
                limit: Limit {
                    input,
                    output,
                    minimumDebit: low,
                    maximumDebit: cap,
                    minimumCredit: minimum,
                    deadline: U256::from(o.expiry),
                },
                signedOrder: Signed {
                    order: o.api.encoded_order.clone(),
                    sig: o.api.signature.clone(),
                },
                quantity,
            }
            .abi_encode(),
        );
        let saved = Saved {
            call: Erc20Call {
                chain: profile.chain,
                payer: owner,
                module: profile.module.address,
                calldata: keccak256(&data),
            },
            profile,
            digest: keccak256(&o.api.encoded_order),
            hash: o.api.order_hash,
            maker: o.order.info.swapper,
            nonce: o.order.info.nonce,
            input,
            output,
            minimum_debit: low,
            maximum_debit: cap,
            minimum_credit: minimum,
            quantity,
        };
        Self { data, saved }
    }
    pub(super) async fn measure(
        &self,
        ctx: &Ctx,
        head: white_state::Head,
        expiry: u64,
    ) -> GatewayResult<(U256, U256, u64)> {
        let p = &self.saved;
        let (debit, credit, gas, logs) = crate::venues::bsc_dex::exec::erc20_measure::run(
            ctx,
            head,
            expiry,
            &p.call,
            (p.input, p.output),
            &self.data,
        )
        .await?;
        if p.amounts(&logs) != Some((debit, credit)) {
            return Err(no(
                ctx,
                "Gladius: фактическая оплата расходится с событиями",
            ));
        }
        Ok((debit, credit, gas))
    }
}
