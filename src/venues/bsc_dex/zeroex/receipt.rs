//! Доказательство исходной заявки. После перезапуска подпись для сверки не требуется.
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

alloy::sol! {
    interface IWallet {
        function settle(address input, address output, uint256 amount, uint256 minimum, uint256 expiry, bytes original) external returns(uint256);
        event Settled(address indexed payer, bytes32 indexed actionHash, address indexed settler, address input, address output, uint256 debit, uint256 credit);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::venues::bsc_dex) struct Saved {
    pub call: Erc20Call,
    pub(super) original: B256,
    pub(super) settler: Address,
    pub(super) input: Address,
    pub(super) output: Address,
    pub(super) debit: U256,
    pub(super) minimum: U256,
    pub(super) expiry: u64,
}

impl Saved {
    pub(in crate::venues::bsc_dex) fn new(
        call: Erc20Call,
        original: B256,
        settler: Address,
        pair: (Address, Address),
        debit: U256,
        minimum: U256,
        expiry: u64,
    ) -> Self {
        Self {
            call,
            original,
            settler,
            input: pair.0,
            output: pair.1,
            debit,
            minimum,
            expiry,
        }
    }
    pub(in crate::venues::bsc_dex) fn wallet(&self, ctx: &Ctx) -> bool {
        ctx.chain_id() == Some(self.call.chain)
            && wallet::signer::address(ctx).ok() == Some(self.call.payer)
    }
    pub(in crate::venues::bsc_dex) fn amounts(&self, logs: &[Log]) -> Option<(U256, U256)> {
        let payer = self.call.payer;
        let net = |token| {
            (
                fill::moved_logs(logs, token, Some(payer), None),
                fill::moved_logs(logs, token, None, Some(payer)),
            )
        };
        let (sent, refund) = net(self.input);
        let (cost, output) = net(self.output);
        let debit = sent.checked_sub(refund)?;
        let credit = output.checked_sub(cost)?;
        let mut events = logs.iter().filter(|l| {
            l.address() == self.call.module
                && l.topics().first() == Some(&IWallet::Settled::SIGNATURE_HASH)
        });
        let e = IWallet::Settled::decode_log_data(&events.next()?.inner.data).ok()?;
        (events.next().is_none()
            && self.call.chain == 1
            && self.input != self.output
            && debit == self.debit
            && credit >= self.minimum
            && (
                e.payer,
                e.actionHash,
                e.settler,
                e.input,
                e.output,
                e.debit,
                e.credit,
            ) == (
                payer,
                self.original,
                self.settler,
                self.input,
                self.output,
                debit,
                credit,
            ))
            .then_some((debit, credit))
    }
    pub(in crate::venues::bsc_dex) async fn measure(
        &self,
        ctx: &Ctx,
        h: Head,
        data: &Bytes,
    ) -> GatewayResult<(U256, u64)> {
        let (debit, credit, gas, logs) = exec::erc20_measure::run(
            ctx,
            h,
            self.expiry,
            &self.call,
            (self.input, self.output),
            data,
        )
        .await?;
        if self.amounts(&logs) != Some((debit, credit)) {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "0x: суммы в квитанции отличаются от остатков плательщика",
            ));
        }
        Ok((credit, gas))
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
                "0x: исходная квитанция не подтвердила оплату",
            ));
        }
        Ok(())
    }
}
