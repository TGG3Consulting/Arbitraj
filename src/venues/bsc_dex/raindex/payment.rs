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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    via: Option<(cash::Role, bool, U256)>,
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
        let book_payment = wire::Settled::decode_log_data(unique(
            self.profile.module,
            wire::Settled::SIGNATURE_HASH,
        )?)
        .ok()?;
        if !empty
            || sender != self.profile.module
            || hash != self.hash
            || i != U256::from(self.i)
            || j != U256::from(self.j)
            || book_payment.orderCall != self.digest
        {
            return None;
        }
        let (spent, received) = if let Some((route, buy, middle)) = self.via {
            let e = cash::Settled::decode_log_data(unique(
                self.call.module,
                cash::Settled::SIGNATURE_HASH,
            )?)
            .ok()?;
            if self.call.module != route.wrapper
                || book_payment.payer != route.wrapper
                || e.owner != self.call.payer
                || e.buy != buy
                || e.action != self.digest
                || e.intermediate.is_zero()
            {
                return None;
            }
            if buy
                && (self.input != route.cash
                    || e.asset != self.output
                    || book_payment.input != route.bridge
                    || book_payment.output != e.asset
                    || book_payment.spent != e.intermediate
                    || e.intermediate > middle
                    || book_payment.received < e.received)
            {
                return None;
            }
            if !buy
                && (self.output != route.cash
                    || e.asset != self.input
                    || book_payment.input != e.asset
                    || book_payment.output != route.bridge
                    || book_payment.received != e.intermediate
                    || e.intermediate < middle
                    || book_payment.spent != e.spent)
            {
                return None;
            }
            (e.spent, e.received)
        } else {
            if book_payment.payer != self.call.payer
                || book_payment.input != self.input
                || book_payment.output != self.output
            {
                return None;
            }
            (book_payment.spent, book_payment.received)
        };
        if spent.is_zero() || spent < self.low || spent > self.cap || received < self.floor {
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
        (net(self.input, true)? == spent && net(self.output, false)? == received)
            .then_some((spent, received))
    }
}
pub(super) struct Payment {
    pub data: Bytes,
    pub saved: Saved,
}
impl Payment {
    #[cfg(test)]
    pub(super) fn new(
        leg: &Leg,
        owner: Address,
        action: Bytes,
        expiry: u64,
        limits: (U256, U256, U256),
    ) -> Self {
        Self::route(leg, owner, action, expiry, limits, U256::ZERO)
    }
    pub(super) fn route(
        leg: &Leg,
        owner: Address,
        action: Bytes,
        expiry: u64,
        (low, cap, floor): (U256, U256, U256),
        middle: U256,
    ) -> Self {
        let digest = keccak256(&action);
        let (input, output) = leg.pair();
        let via = leg
            .via
            .map(|b| (b.role, leg.input == b.role.bridge, middle));
        let data = Bytes::from(if let Some((_, buy, bridge_limit)) = via {
            cash::fillCall {
                r: cash::Request {
                    asset: if buy { output } else { input },
                    cap,
                    floor,
                    bridgeLimit: bridge_limit,
                    deadline: U256::from(expiry),
                    buy,
                },
                action,
            }
            .abi_encode()
        } else {
            wire::fillCall {
                l: wire::Limits {
                    input,
                    output,
                    minimumDebit: low,
                    maximumDebit: cap,
                    minimumCredit: floor,
                    deadline: U256::from(expiry),
                },
                action,
            }
            .abi_encode()
        });
        let saved = Saved {
            via,
            call: Erc20Call {
                chain: leg.p.chain,
                payer: owner,
                module: leg.target(),
                calldata: keccak256(&data),
            },
            profile: leg.p,
            digest,
            hash: leg.hash,
            i: leg.i,
            j: leg.j,
            input,
            output,
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
