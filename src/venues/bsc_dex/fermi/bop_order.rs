//! Сохранённый путь bopAMM, точная форма подписи и проверка конечной выплаты после включения.
use super::{
    Ctx,
    bop_binding::Identity,
    bop_cash::{CashOrder, executeCall, executeCashCall},
    bopamm_profile::Role,
    slot,
};
use crate::gateway::errors::GatewayResult;
use alloy::primitives::{Address, B256, Bytes, U256};
use alloy::sol_types::SolCall;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct Order {
    pub(super) source: Identity,
    pub(super) bridge: bool,
    pub(super) paid: U256,
    pub(super) minimum: U256,
    pub(super) runtime: B256,
}
impl Order {
    pub(super) async fn verify_at(
        &self,
        ctx: &Ctx,
        h: B256,
        executor: Address,
    ) -> GatewayResult<()> {
        self.source
            .verify(
                ctx,
                h,
                Role {
                    address: executor,
                    hash: self.runtime,
                },
                self.runtime == super::bop_versions::CASH,
            )
            .await?;
        if self.bridge {
            self.source.verify_bridge(ctx, h).await?;
        }
        Ok(())
    }
    pub(super) fn calldata(&self, p: &slot::Proof) -> Bytes {
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
    pub(super) fn matches(&self, p: &slot::Proof, spent: U256, received: U256) -> bool {
        let buy = p.amount.is_negative();
        let c = &self.source.cash;
        let pair = if buy {
            (c.usdt.address, c.weth.address)
        } else {
            (c.weth.address, c.usdt.address)
        };
        (p.input, p.output) == pair
            && p.router == Some(self.source.market.address)
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
