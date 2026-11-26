//! Verify the included official V2 multicall and its unique Core swap during normal completion and recovery.
use super::super::{
    ctx::Ctx,
    node::rpc,
    pools::{abi_ekubo, ekubo_v2::OrderKey},
    send::track::Receipt,
};
use super::{
    fill,
    plan::{DirectCall, Plan},
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::rpc::types::Log;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
    pub core: Address,
    pub router: Address,
    pub payer: Address,
    pub wrapped: Address,
    pub key: OrderKey,
    pub minimum: U256,
    pub deadline: U256,
    pub data_hash: B256,
}
impl Proof {
    pub(super) fn of(p: &Plan, deadline: U256) -> Option<Self> {
        let DirectCall::EkuboV2 {
            key,
            core,
            wrapped: Some(wrapped),
        } = p.direct_market?
        else {
            return None;
        };
        Some(Self {
            core,
            router: p.router,
            payer: p.wallet,
            wrapped,
            key,
            minimum: p.min_total(),
            deadline,
            data_hash: keccak256(p.data(U256::ZERO)),
        })
    }
    fn data(&self) -> Bytes {
        self.key.calldata(self.minimum, self.deadline)
    }
    pub(crate) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Tx {
            hash: B256,
            block_hash: Option<B256>,
            from: Address,
            to: Option<Address>,
            input: Bytes,
            value: U256,
        }
        let bad = || {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "Ekubo V2: исходный hash, блок или реквизиты подписанного вызова не совпали",
            )
        };
        if r.transaction_hash != Some(hash) || r.block_hash.is_none() {
            return Err(bad());
        }
        let t: Option<Tx> = rpc::call(ctx, "eth_getTransactionByHash", json!([hash]))
            .await
            .map_err(|_| bad())?;
        if t.is_some_and(|t| {
            t.hash == hash
                && t.block_hash == r.block_hash
                && t.from == self.payer
                && t.to == Some(self.router)
                && t.value == self.key.value()
                && keccak256(&t.input) == self.data_hash
                && t.input == self.data()
        }) {
            Ok(())
        } else {
            Err(bad())
        }
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        if wallet != self.payer || !self.key.token0.is_zero() || self.wrapped.is_zero() {
            return None;
        }
        let (expected_in, expected_out) = if self.key.side {
            (self.key.token1, self.wrapped)
        } else {
            (self.wrapped, self.key.token1)
        };
        if (input, output) != (expected_in, expected_out) {
            return None;
        }
        let id = abi_ekubo::pool_id(self.key.token0, self.key.token1, self.key.config);
        let mut rows = logs
            .iter()
            .filter(|r| r.address() == self.core && r.topics().is_empty())
            .filter_map(|l| {
                let data = &l.inner.data.data;
                let (pool, balance, _) = abi_ekubo::decode_swap_log(data)?;
                (pool == id && data.get(..20) == Some(self.router.as_slice()))
                    .then(|| abi_ekubo::decode_balance(&balance))
            });
        let (d0, d1) = rows.next()?;
        if rows.next().is_some() {
            return None;
        }
        let (used, paid) = if self.key.side { (d1, d0) } else { (d0, d1) };
        if used != self.key.amount || used <= 0 || paid >= 0 {
            return None;
        }
        let debit = U256::from(used.unsigned_abs());
        let wcredit = U256::from(paid.unsigned_abs());
        let incoming = fill::moved_logs(logs, self.key.token1, None, Some(wallet));
        let outgoing = fill::moved_logs(logs, self.key.token1, Some(wallet), None);
        let credited = if self.key.side {
            if outgoing.checked_sub(incoming)? != debit {
                return None;
            }
            wcredit
        } else {
            incoming.checked_sub(outgoing)?
        };
        (credited >= self.minimum && !credited.is_zero()).then_some((debit, credited))
    }
}
