//! Original Fluid DexLite LogSwap identifies the physical market and direction. The wallet event records the actual ERC20 quantities.
use super::signed_call;
use crate::{
    core::types::fluid_dex_lite::Market,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        check::simulate::SimLog, ctx::Ctx, pools::solidly_classic_factory::plain,
        quote::v2_wallet_sim::Order, send::receipt::Receipt,
    },
};
use alloy::sol_types::SolCall;
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
            function hopVersion() external pure returns(uint256);
            function executeHop(bytes32[] markets,address[] path,uint256 amountIn,uint256 minimum,uint256 deadline) external returns(uint256 spent,uint256 received);
            event HopFilled(bytes32 indexed route,address indexed payer,address indexed recipient,address input,address output,uint256 spent,uint256 received);
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<super::fluid_lite_hop::Route>,
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
            route: None,
            payer: o.payer,
            input: o.input,
            output: o.output,
            maximum: o.amount,
            minimum: self.desired.unwrap_or(minimum),
            deadline,
        }
    }
}
pub(super) fn scale(amount: U256, from: u8, to: u8) -> Option<U256> {
    let factor = U256::from(10).checked_pow(U256::from(from.abs_diff(to)))?;
    if from <= to {
        amount.checked_mul(factor)
    } else {
        Some(amount / factor)
    }
}
impl Proof {
    pub(in crate::venues::bsc_dex) fn data(&self) -> Bytes {
        if let Some(route) = &self.route {
            return route.data(self).unwrap_or_default();
        }
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
}

#[cfg(test)]
#[path = "fluid_lite_tests.rs"]
mod tests;
