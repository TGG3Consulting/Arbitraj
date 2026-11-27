//! Факт одной оплаты связывает исходную транзакцию, оба события и чистые переводы кошелька.
use super::*;
use crate::venues::bsc_dex::{
    exec::{erc20_measure, signed_call::Erc20Call},
    node::abi::IERC20,
    send::receipt::Receipt,
    wallet,
};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent, SolValue},
};
use alloy_primitives::keccak256;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Saved {
    call: Erc20Call,
    profile: Profile,
    digest: B256,
    hash: B256,
    i: usize,
    j: usize,
    input: Address,
    output: Address,
    low: U256,
    cap: U256,
    floor: U256,
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
        if r.status.to::<u64>() == 1 && self.amounts(&r.logs).is_none() {
            return Err(ctx.err(
                ErrorCode::OutcomeUnknown,
                "Raindex: квитанция не подтверждает исходный ордер и платёж",
            ));
        }
        Ok(())
    }
    pub(in crate::venues::bsc_dex) fn amounts(&self, logs: &[Log]) -> Option<(U256, U256)> {
        let unique = |address, topic| {
            let mut rows = logs
                .iter()
                .filter(|l| l.address() == address && l.topics().first() == Some(&topic));
            let row = rows.next()?;
            rows.next().is_none().then_some(&row.inner.data)
        };
        let (sender, hash, i, j, empty) = match self.profile.version {
            4 => {
                let e = wire::TakeOrderV2::decode_log_data(unique(
                    self.profile.book,
                    wire::TakeOrderV2::SIGNATURE_HASH,
                )?)
                .ok()?;
                let c = e.config;
                (
                    e.sender,
                    keccak256(c.order.abi_encode()),
                    c.inputIOIndex,
                    c.outputIOIndex,
                    c.signedContext.is_empty(),
                )
            }
            6 => {
                let e = wire::TakeOrderV3::decode_log_data(unique(
                    self.profile.book,
                    wire::TakeOrderV3::SIGNATURE_HASH,
                )?)
                .ok()?;
                let c = e.config;
                (
                    e.sender,
                    keccak256(c.order.abi_encode()),
                    c.inputIOIndex,
                    c.outputIOIndex,
                    c.signedContext.is_empty(),
                )
            }
            _ => return None,
        };
        let e = wire::Settled::decode_log_data(unique(
            self.call.module,
            wire::Settled::SIGNATURE_HASH,
        )?)
        .ok()?;
        if !empty
            || sender != self.call.module
            || hash != self.hash
            || i != U256::from(self.i)
            || j != U256::from(self.j)
            || e.payer != self.call.payer
            || e.orderCall != self.digest
            || e.input != self.input
            || e.output != self.output
            || e.spent.is_zero()
            || e.spent < self.low
            || e.spent > self.cap
            || e.received < self.floor
        {
            return None;
        }
        let net = |address, debit| {
            let mut incoming = U256::ZERO;
            let mut outgoing = U256::ZERO;
            for l in logs.iter().filter(|l| {
                l.address() == address
                    && l.topics().first() == Some(&IERC20::Transfer::SIGNATURE_HASH)
            }) {
                let t = IERC20::Transfer::decode_log_data(&l.inner.data).ok()?;
                if t.to == self.call.payer {
                    incoming = incoming.checked_add(t.value)?;
                }
                if t.from == self.call.payer {
                    outgoing = outgoing.checked_add(t.value)?;
                }
            }
            if debit {
                outgoing.checked_sub(incoming)
            } else {
                incoming.checked_sub(outgoing)
            }
        };
        (net(self.input, true)? == e.spent && net(self.output, false)? == e.received)
            .then_some((e.spent, e.received))
    }
}
pub(super) struct Payment {
    pub data: Bytes,
    pub saved: Saved,
}
impl Payment {
    pub(super) fn new(
        leg: &Leg,
        owner: Address,
        action: Bytes,
        expiry: u64,
        (low, cap, floor): (U256, U256, U256),
    ) -> Self {
        let digest = keccak256(&action);
        let data = Bytes::from(
            wire::fillCall {
                l: wire::Limits {
                    input: leg.input,
                    output: leg.output,
                    minimumDebit: low,
                    maximumDebit: cap,
                    minimumCredit: floor,
                    deadline: U256::from(expiry),
                },
                action,
            }
            .abi_encode(),
        );
        let saved = Saved {
            call: Erc20Call {
                chain: leg.p.chain,
                payer: owner,
                module: leg.p.module,
                calldata: keccak256(&data),
            },
            profile: leg.p,
            digest,
            hash: leg.hash,
            i: leg.i,
            j: leg.j,
            input: leg.input,
            output: leg.output,
            low,
            cap,
            floor,
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
        let (debit, credit, gas, logs) =
            erc20_measure::run(ctx, head, expiry, &p.call, (p.input, p.output), &self.data).await?;
        if p.amounts(&logs) != Some((debit, credit)) {
            return Err(no(
                ctx,
                "Raindex: фактические движения расходятся с событиями",
            ));
        }
        Ok((debit, credit, gas))
    }
}
