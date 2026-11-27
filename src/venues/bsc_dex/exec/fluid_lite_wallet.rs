//! Original Fluid DexLite LogSwap identifies the physical market and direction. The wallet event records the actual ERC20 quantities.
use super::{fill, signed_call};
use crate::{
    core::types::fluid_dex_lite::Market,
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
        struct FluidKey{address token0;address token1;bytes32 salt;}
        interface IWallet{
            function dex() external view returns(address);

            function wrapped() external view returns(address);
            function venueHash() external view returns(bytes32);
            function resolve(bytes32 id) external view returns(FluidKey);
            function execute(bytes32 market,address input,address output,uint256 budget,uint256 minimum,bool exactOut,uint256 deadline) external returns(uint256 spent,uint256 received);
            event LogSwap(uint256 swapData,uint256 dexVariables);
            event Filled(bytes32 indexed market,address indexed payer,address indexed recipient,address input,address output,uint256 spent,uint256 received);
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct Terms {
    pub market: Market,
    pub dex: Address,
    pub wrapped: Address,
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
            maximum: o.amount,
            minimum: self.desired.unwrap_or(minimum),
            deadline,
        }
    }
}
fn scale(amount: U256, from: u8, to: u8) -> Option<U256> {
    let factor = U256::from(10).checked_pow(U256::from(from.abs_diff(to)))?;
    if from <= to {
        amount.checked_mul(factor)
    } else {
        Some(amount / factor)
    }
}
impl Proof {
    pub(in crate::venues::bsc_dex) fn data(&self) -> Bytes {
        let Some(market) = self.terms.market.id() else {
            return Bytes::new();
        };
        abi::IWallet::executeCall {
            market,
            input: self.input,
            output: self.output,
            budget: self.maximum,
            minimum: self.minimum,
            exactOut: self.terms.desired.is_some(),
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
                "Fluid DEX Lite: отсутствует блок квитанции",
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
                != (self.terms.market.id()?, payer, payer, input, output)
            || ev.spent.is_zero()
            || ev.spent > self.maximum
            || ev.received < self.minimum
        {
            return None;
        }
        let mut trades = logs.iter().filter(|l| {
            l.address() == self.terms.dex
                && l.topic0() == Some(&abi::IWallet::LogSwap::SIGNATURE_HASH)
        });
        let event = abi::IWallet::LogSwap::decode_log_data(&trades.next()?.inner.data).ok()?;
        let market = self.terms.market;
        let pair = market.tokens(self.terms.wrapped)?;
        let forward = (input, output) == (pair[0], pair[1]);
        if trades.next().is_some()
            || (!forward && (input, output) != (pair[1], pair[0]))
            || event.swapData.bit(64) != forward
            || (event.swapData & U256::from(u64::MAX)) != U256::from_be_slice(&market.id()?[..8])
            || (self.terms.desired.is_none() && ev.spent != self.maximum)
        {
            return None;
        }
        // The core emits both sides at nine decimals. The fixed leg is truncated into that
        // precision first; the computed leg is truncated back into its underlying token.
        let mask = (U256::ONE << 60) - U256::ONE;
        let ix = if forward { 126 } else { 131 };
        let ox = if forward { 131 } else { 126 };
        let decimals = |shift: usize| ((event.dexVariables >> shift) & U256::from(31)).to::<u8>();
        let spent9 = (event.swapData >> 65) & mask;
        let received9 = (event.swapData >> 125) & mask;
        if self.terms.desired.is_some() {
            if scale(spent9, 9, decimals(ix))? != ev.spent
                || scale(self.minimum, decimals(ox), 9)? != received9
            {
                return None;
            }
        } else if scale(self.maximum, decimals(ix), 9)? != spent9
            || scale(received9, 9, decimals(ox))?.is_zero()
        {
            return None;
        }
        let moved = |t, f, to| fill::moved_logs(logs, t, f, to);
        let debit = moved(input, Some(payer), None).checked_sub(moved(input, None, Some(payer)))?;
        let net = moved(output, None, Some(payer)).checked_sub(moved(output, Some(payer), None))?;
        let native = Address::repeat_byte(0xee);
        let (pin, pout) = if forward {
            (market.token0, market.token1)
        } else {
            (market.token1, market.token0)
        };
        let origin = if pout == native {
            self.terms.target
        } else {
            self.terms.dex
        };
        let payee = if pin == native {
            self.terms.target
        } else {
            self.terms.dex
        };
        (moved(input, Some(payer), Some(payee)) > U256::ZERO
            && moved(output, Some(origin), Some(payer)) > U256::ZERO
            && (debit, net) == (ev.spent, ev.received))
            .then_some((debit, net))
    }
}

#[cfg(test)]
#[path = "fluid_lite_tests.rs"]
mod tests;
