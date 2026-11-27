//! Подписанный вызов Motoswap и проверка канонической квитанции после перезапуска.
//! Исполнитель и восстановление ордера используют сохранённые условия, ABI и квитанцию исходной транзакции.
#[path = "motoswap_payment.rs"]
mod payment;

use super::{carbon_binding::Code, signed_call};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx, pools::solidly_classic_factory,
        quote::v2_wallet_sim::Order, send::track::Receipt,
    },
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};

#[allow(clippy::too_many_arguments)]
pub(in crate::venues::bsc_dex) mod abi {
    alloy::sol! {
        interface IModule {
            function execute(address pool,address input,address output,uint256 amount,uint256 minimum,uint256 deadline) external returns(uint256);
            function pathVersion() external view returns(uint256);
            function executePath(address[] pools,address[] path,uint256 amount,uint256 minimum,uint256 deadline) external returns(uint256);
            event PathPaid(bytes32 indexed route,address indexed payer,address indexed recipient,address input,address output,uint256 spent,uint256 received);
            function factory() external view returns(address);
            function router() external view returns(address);
            event Paid(address indexed pool,address indexed payer,address indexed recipient,address input,address output,uint256 spent,uint256 received);
        }
        interface IFee {
            function swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256 amountIn,uint256 amountOutMin,address[] path,address to,uint256 deadline) external returns(uint256);
            event Swap(address indexed user,address indexed tokenIn,address indexed tokenOut,uint256 amountIn,uint256 amountOut,address feeToken,uint256 feeAmount);
        }
        interface IPair {event Swap(address indexed sender,uint256 amount0In,uint256 amount1In,uint256 amount0Out,uint256 amount1Out,address indexed to);}
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Terms {
    pub pool: Address,
    pub target: Address,
    pub fee: Address,
    pub core: Address,
    pub fee_code: Code,
    pub module_hash: Option<B256>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub pools: Vec<Address>,
    pub path: Vec<Address>,
}
impl Route {
    pub(in crate::venues::bsc_dex) fn proof(
        &self,
        t: Terms,
        o: &Order,
        minimum: U256,
        deadline: U256,
    ) -> Proof {
        let mut p = t.proof(o, minimum, deadline);
        p.route = Some(self.clone());
        p
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<Route>,
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
            route: None,
            terms: self,
            payer: o.payer,
            input: o.input,
            output: o.output,
            maximum: o.amount,
            minimum,
            deadline,
        }
    }
}
impl Proof {
    pub fn data(&self) -> Bytes {
        if let Some(route) = &self.route {
            return if self.terms.module_hash.is_some() {
                abi::IModule::executePathCall {
                    pools: route.pools.clone(),
                    path: route.path.clone(),
                    amount: self.maximum,
                    minimum: self.minimum,
                    deadline: self.deadline,
                }
                .abi_encode()
                .into()
            } else {
                abi::IFee::swapExactTokensForTokensSupportingFeeOnTransferTokensCall {
                    amountIn: self.maximum,
                    amountOutMin: self.minimum,
                    path: route.path.clone(),
                    to: self.payer,
                    deadline: self.deadline,
                }
                .abi_encode()
                .into()
            };
        }
        if self.terms.module_hash.is_some() {
            abi::IModule::executeCall {
                pool: self.terms.pool,
                input: self.input,
                output: self.output,
                amount: self.maximum,
                minimum: self.minimum,
                deadline: self.deadline,
            }
            .abi_encode()
            .into()
        } else {
            abi::IFee::swapExactTokensForTokensSupportingFeeOnTransferTokensCall {
                amountIn: self.maximum,
                amountOutMin: self.minimum,
                path: vec![self.input, self.output],
                to: self.payer,
                deadline: self.deadline,
            }
            .abi_encode()
            .into()
        }
    }
    pub(super) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            hash,
            r,
            self.payer,
            self.terms.target,
            U256::ZERO,
            self.data(),
        )
        .await?;
        self.terms
            .fee_code
            .at_transaction(ctx, self.terms.fee, hash, r, None)
            .await?;
        if let Some(expected) = self.terms.module_hash {
            let block = r.block_hash.ok_or_else(|| {
                ctx.err(ErrorCode::OutcomeUnknown, "Motoswap: нет блока включения")
            })?;
            solidly_classic_factory::plain(
                ctx,
                self.terms.target,
                expected,
                &serde_json::json!({"blockHash":block,"requireCanonical":true}),
            )
            .await?;
        }
        crate::venues::bsc_dex::send::canonical_receipt::verify(ctx, hash, r).await
    }
}

#[cfg(test)]
#[path = "motoswap_wallet_tests.rs"]
mod tests;
