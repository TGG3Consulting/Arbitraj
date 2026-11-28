//! Сохранённый путь bopAMM, точная форма подписи и проверка конечной выплаты после включения.
use super::{
    Ctx,
    bop_cash::{CashOrder, MARKET, USDT, WETH, bound_profile, executeCall, executeCashCall},
    bopamm_profile::Role,
    slot,
};
use crate::gateway::errors::GatewayResult;
use alloy::primitives::{Address, B256, Bytes, U256};
use alloy::sol_types::SolCall;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct Order {
    pub(super) bridge: bool,
    pub(super) paid: U256,
    pub(super) minimum: U256,
    pub(super) runtime: B256,
}
impl Order {
    pub(super) async fn verify_at(
        self,
        ctx: &Ctx,
        h: B256,
        executor: Address,
    ) -> GatewayResult<()> {
        bound_profile()
            .verify(
                ctx,
                h,
                Some(Role {
                    address: executor,
                    hash: self.runtime,
                }),
            )
            .await
    }
    pub(super) fn calldata(self, p: &slot::Proof) -> Bytes {
        let o = CashOrder {
            leg: p.leg,
            parent: p.parent,
            height: p.target,
            timestamp: p.timestamp,
            input: p.input,
            output: p.output,
            amount: self.paid,
            minimum: self.minimum,
        };
        if self.bridge {
            executeCashCall { o }.abi_encode()
        } else {
            executeCall { o }.abi_encode()
        }
        .into()
    }
    pub(super) fn matches(self, p: &slot::Proof, spent: U256, received: U256) -> bool {
        let buy = p.amount.is_negative();
        let pair = if buy { (USDT, WETH) } else { (WETH, USDT) };
        (p.input, p.output) == pair
            && p.router == Some(MARKET)
            && !p.executor.is_zero()
            && !p.payer.is_zero()
            && (self.runtime == super::bop_versions::CASH
                || (!self.bridge && self.runtime == super::bop_versions::DIRECT))
            && !self.minimum.is_zero()
            && spent == self.paid
            && received >= self.minimum
            && (if buy {
                self.minimum == p.amount.unsigned_abs() && spent == p.limit
            } else {
                spent == p.amount.unsigned_abs() && self.minimum == p.limit
            })
    }
}
