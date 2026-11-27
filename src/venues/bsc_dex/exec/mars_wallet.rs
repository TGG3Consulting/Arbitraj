//! MarSwap. Квитанция платного обмена между ERC20 и отдельно подписанное количество ETH.
//! Пара может выпустить дополнительные события при внутренней продаже налога исходным токеном.
use super::{fill, fork_swap, signed_call};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        check::simulate::SimLog, ctx::Ctx, pools::abi_v2v3::IV2Pair, quote::v2_wallet_sim::Order,
        send::track::Receipt,
    },
};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Terms {
    pub pair: Address,
    pub router: Address,
    pub wei: U256,
    pub router_hash: B256,
    pub pair_hash: B256,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub market: Terms,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Terms {
    pub(in crate::venues::bsc_dex) fn proof(self, o: &Order, min: U256, deadline: U256) -> Proof {
        Proof {
            market: self,
            payer: o.payer,
            input: o.input,
            output: o.output,
            amount: o.amount,
            minimum: min,
            deadline,
        }
    }
}
impl Proof {
    pub(in crate::venues::bsc_dex) fn data(&self) -> Bytes {
        fork_swap::swap_data(
            self.amount,
            self.minimum,
            &[self.input, self.output],
            self.payer,
            self.deadline,
        )
    }
    pub(super) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        let block = r.block_hash.ok_or_else(|| {
            ctx.err(
                crate::gateway::errors::ErrorCode::OutcomeUnknown,
                "MarSwap: не подтверждён блок исполнения",
            )
        })?;
        let at = serde_json::json!({"blockHash":block,"requireCanonical":true});
        let m = self.market;
        use crate::venues::bsc_dex::pools::solidly_classic_factory::plain;
        tokio::try_join!(
            signed_call::included(ctx, hash, r, self.payer, m.router, m.wei, self.data()),
            plain(ctx, m.router, m.router_hash, &at),
            plain(ctx, m.pair, m.pair_hash, &at)
        )
        .map_err(|e| ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown, e.message))?;
        crate::venues::bsc_dex::send::canonical_receipt::verify(ctx, hash, r).await
    }
    pub(in crate::venues::bsc_dex) fn measured(&self, rows: &[SimLog]) -> Option<(U256, U256)> {
        self.amounts(
            &signed_call::logs(rows)?,
            self.payer,
            self.input,
            self.output,
        )
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        owner: Address,
        i: Address,
        o: Address,
    ) -> Option<(U256, U256)> {
        let m = self.market;
        if (owner, i, o) != (self.payer, self.input, self.output)
            || i == o
            || owner == m.pair
            || m.pair == m.router
        {
            return None;
        }
        let mut target = logs
            .iter()
            .filter(|l| l.address() == m.pair && l.topic0() == Some(&IV2Pair::Swap::SIGNATURE_HASH))
            .map(|l| IV2Pair::Swap::decode_log_data(&l.inner.data).ok())
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .filter(|s| s.to == owner);
        let s = target.next()?;
        if target.next().is_some() || s.sender != m.router {
            return None;
        }
        let (gross, opposite_in, opposite_out) = if i < o {
            (s.amount1Out, s.amount1In, s.amount0Out)
        } else {
            (s.amount0Out, s.amount0In, s.amount1Out)
        };
        if gross.is_zero() || !opposite_in.is_zero() || !opposite_out.is_zero() {
            return None;
        }
        let moved = |coin, src, dst| fill::moved_logs(logs, coin, src, dst);
        let debit = moved(i, Some(owner), None).checked_sub(moved(i, None, Some(owner)))?;
        let credit = moved(o, None, Some(owner)).checked_sub(moved(o, Some(owner), None))?;
        if debit != self.amount
            || moved(i, Some(owner), Some(m.pair)).is_zero()
            || credit.is_zero()
            || credit < self.minimum
            || credit > gross
            || moved(o, Some(m.pair), Some(owner)) != credit
        {
            return None;
        }
        Some((debit, credit))
    }
}

#[cfg(test)]
#[path = "mars_wallet_tests.rs"]
mod tests;
