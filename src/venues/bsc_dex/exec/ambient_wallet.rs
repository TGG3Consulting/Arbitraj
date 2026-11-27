//! Receipt of the specific owner-funded transaction, including any returned input.
//! The protected market returns WETH to the wallet for a native Ambient leg.
use super::{fill, signed_call};
use crate::{
    core::types::ambient::Market,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        check::simulate::SimLog, ctx::Ctx, pools::solidly_classic_factory::plain,
        quote::v2_wallet_sim::Order, send::receipt::Receipt,
    },
};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};
#[allow(clippy::too_many_arguments)]
pub(in crate::venues::bsc_dex) mod abi {
    alloy::sol! {
        struct AmbientKey{address base;address quote;uint256 index;}
        interface IWallet{
            function dex() external view returns(address);
            function query() external view returns(address);
            function wrapped() external view returns(address);
            function hot() external view returns(address);
            function knockout() external view returns(address);
            function venueHash() external view returns(bytes32);
            function hotHash() external view returns(bytes32);
            function knockoutHash() external view returns(bytes32);
            function resolve(bytes32 id,address src,address dst) external view returns(AmbientKey);
            function execute(AmbientKey market,address input,address output,uint128 budget,uint128 minimum,bool exactOut,uint128 limitPrice,uint256 deadline) external returns(uint256 spent,uint256 received);
            event Filled(bytes32 indexed market,address indexed payer,address indexed recipient,address input,address output,uint256 spent,uint256 received);
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct Terms {
    pub market: Market,
    pub target: Address,
    pub module_hash: B256,
    pub desired: Option<U256>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Proof {
    pub terms: Terms,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub maximum: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Terms {
    pub(in crate::venues::bsc_dex) fn proof(self, o: &Order, minimum: U256, deadline: U256) -> Proof {
        Proof {
            terms: self,
            payer: o.payer,
            input: o.input,
            output: o.output,
            maximum: o.amount,
            minimum: self.desired.unwrap_or(minimum),
            deadline,
        }
    }
}
impl Proof {
    pub(in crate::venues::bsc_dex) fn data(&self) -> Bytes {
        let m = self.terms.market;
        let to_amount = |n: U256| {
            u128::try_from(n)
                .ok()
                .filter(|v| *v <= i128::MAX as u128)
                .unwrap_or(0)
        };
        abi::IWallet::executeCall {
            market: abi::AmbientKey {
                base: m.base,
                quote: m.quote,
                index: m.index,
            },
            input: self.input,
            output: self.output,
            budget: to_amount(self.maximum),
            minimum: to_amount(self.minimum),
            exactOut: self.terms.desired.is_some(),
            limitPrice: 0,
            deadline: self.deadline,
        }
        .abi_encode()
        .into()
    }
    pub(super) async fn included(
        &self,
        ctx: &Ctx,
        hash: B256,
        receipt: &Receipt,
    ) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            hash,
            receipt,
            self.payer,
            self.terms.target,
            U256::ZERO,
            self.data(),
        )
        .await?;
        let block = receipt.block_hash.ok_or_else(|| {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "Ambient: отсутствует блок квитанции",
            )
        })?;
        plain(
            ctx,
            self.terms.target,
            self.terms.module_hash,
            &serde_json::json!({"blockHash":block,"requireCanonical":true}),
        )
        .await?;
        crate::venues::bsc_dex::send::canonical_receipt::verify(ctx, hash, receipt).await
    }
    pub(crate) fn measured(&self, logs: &[SimLog]) -> Option<(U256, U256)> {
        self.amounts(
            &signed_call::logs(logs)?,
            self.payer,
            self.input,
            self.output,
        )
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        payer: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        if (payer, input, output) != (self.payer, self.input, self.output)
            || self.minimum.is_zero()
            || input == output
        {
            return None;
        }
        let mut rows = logs.iter().filter(|v| {
            v.address() == self.terms.target
                && v.topic0() == Some(&abi::IWallet::Filled::SIGNATURE_HASH)
        });
        let ev = abi::IWallet::Filled::decode_log_data(&rows.next()?.inner.data).ok()?;
        if rows.next().is_some()
            || (ev.market, ev.payer, ev.recipient, ev.input, ev.output)
                != (self.terms.market.id(), payer, payer, input, output)
            || ev.spent.is_zero()
            || ev.spent > self.maximum
            || ev.received < self.minimum
        {
            return None;
        }
        let moved = |t, f, to| fill::moved_logs(logs, t, f, to);
        let debit = moved(input, Some(payer), None).checked_sub(moved(input, None, Some(payer)))?;
        let net = moved(output, None, Some(payer)).checked_sub(moved(output, Some(payer), None))?;
        (moved(input, Some(payer), Some(self.terms.target)) > U256::ZERO
            && moved(output, Some(self.terms.target), Some(payer)) > U256::ZERO
            && (debit, net) == (ev.spent, ev.received))
            .then_some((debit, net))
    }
}

#[cfg(test)]
#[path = "ambient_tests.rs"]
mod tests;
