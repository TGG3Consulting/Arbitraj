//! Отдельные лестницы двух направлений ElfomoFi в одном состоянии сети.
use super::{IElfomo, market};
use crate::core::types::LadderPoint;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{
    ctx::Ctx,
    node::{call, rpc},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};

pub(super) struct Reader<'a> {
    pub ctx: &'a Ctx,
    pub tag: &'a Value,
    pub wallet: Address,
    pub target: Address,
    pub router: Address,
    pub counter: Address,
    pub coin: Address,
}

impl Reader<'_> {
    async fn sell_input(&self, quote_amount: U256) -> GatewayResult<U256> {
        let data = Bytes::from(
            IElfomo::getAmountInCall {
                fromToken: self.coin,
                toToken: self.counter,
                toAmount: quote_amount,
            }
            .abi_encode(),
        );
        // Используется только размер входа. Выплата всегда запрашивается фактическим путём кошелька.
        let caller = if self.target == self.router {
            self.wallet
        } else {
            self.target
        };
        let raw: Bytes = rpc::call(
            self.ctx,
            "eth_call",
            json!([{"from": caller, "to": self.router, "data":data}, self.tag]),
        )
        .await?;
        call::decode::<IElfomo::getAmountInCall>(&raw)
    }

    async fn point(&self, buy: bool, size: U256) -> GatewayResult<LadderPoint> {
        let (from, to, input) = if buy {
            (self.counter, self.coin, size)
        } else {
            (self.coin, self.counter, self.sell_input(size).await?)
        };
        let output = if input.is_zero() {
            U256::ZERO
        } else {
            market::ask(
                self.ctx,
                from,
                to,
                input,
                self.tag,
                self.wallet,
                self.target,
            )
            .await?
        };
        Ok(LadderPoint {
            amount_in: input,
            amount_out: output,
        })
    }

    pub async fn side(
        &self,
        buy: bool,
        sizes: &[U256],
        capacity: U256,
    ) -> GatewayResult<Vec<LadderPoint>> {
        let mut points: Vec<LadderPoint> = Vec::new();
        if capacity.is_zero() {
            return Ok(points);
        }
        for &size in sizes {
            let point = match self.point(buy, size).await {
                Ok(p) => p,
                Err(e) if e.code == ErrorCode::SwapReverted => break,
                Err(e) => return Err(e),
            };
            if !market::within_capacity(point.amount_out, capacity)
                || points.last().is_some_and(|p| {
                    point.amount_in <= p.amount_in || point.amount_out <= p.amount_out
                })
            {
                break;
            }
            points.push(point);
        }
        Ok(points)
    }
}
