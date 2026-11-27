//! Окончательный исход прямой RFQT по первоначальному хешу, подписанным полям и позиции в блоке.
use super::{ReceiptProof, wire};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::rpc,
        send::{canonical_receipt, receipt::Receipt},
    },
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U64, U256, keccak256};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(super) struct Original {
    pub(super) chain: u64,
    pub(super) target: Address,
    pub(super) calldata: B256,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Transaction {
    hash: B256,
    chain_id: U64,
    from: Address,
    to: Address,
    nonce: U64,
    value: U256,
    input: Bytes,
    block_hash: Option<B256>,
    transaction_index: Option<U64>,
}
impl Transaction {
    fn id(&self, chain: u64, hash: B256, from: Address, nonce: u64) -> bool {
        self.hash == hash
            && self.chain_id.to::<u64>() == chain
            && self.from == from
            && self.nonce.to::<u64>() == nonce
    }
    fn position(&self, hash: B256, r: &Receipt) -> bool {
        r.transaction_hash == Some(hash)
            && r.block_hash.is_some()
            && r.transaction_index.is_some()
            && self.block_hash == r.block_hash
            && self.transaction_index == r.transaction_index
    }
    fn cancelled(&self, chain: u64, hash: B256, owner: Address, nonce: u64) -> bool {
        self.id(chain, hash, owner, nonce)
            && self.to == owner
            && self.value.is_zero()
            && self.input.is_empty()
    }
}
impl ReceiptProof {
    pub(super) fn original_matches(
        &self,
        t: &Transaction,
        chain: u64,
        hash: B256,
        nonce: u64,
    ) -> bool {
        if !t.id(chain, hash, self.trader, nonce)
            || t.value
                != if self.input.is_zero() {
                    self.amount
                } else {
                    U256::ZERO
                }
        {
            return false;
        }
        if let Some(o) = self.original {
            return chain == o.chain && t.to == o.target && keccak256(&t.input) == o.calldata;
        }
        // У старых записей нет дайджеста calldata. Изначальный tx hash криптографически
        // фиксирует подпись. Восстанавливаем согласованные публичные поля из полного вызова.
        if super::super::router(chain).ok() != Some(t.to) {
            return false;
        }
        let Ok(call) = wire::tradeRFQTCall::abi_decode_validate(&t.input) else {
            return false;
        };
        if call.abi_encode().as_slice() != t.input.as_ref() {
            return false;
        }
        let q = call.quote;
        q.pool == self.pool
            && q.trader == self.trader
            && q.effectiveTrader == self.trader
            && q.txid == self.txid
            && q.baseToken == self.input
            && q.quoteToken == self.output
            && q.effectiveBaseTokenAmount == self.amount
            && !self.amount.is_zero()
            && self.amount <= q.baseTokenAmount
            && !q.signature.is_empty()
            && q.quoteTokenAmount
                .checked_mul(self.amount)
                .and_then(|n| n.checked_div(q.baseTokenAmount))
                == Some(self.proceeds)
    }
    pub(in crate::venues::bsc_dex) async fn included(
        &self,
        ctx: &Ctx,
        hash: B256,
        nonce: u64,
        r: &Receipt,
    ) -> GatewayResult<()> {
        let bad = || {
            ctx.err(ErrorCode::OutcomeUnknown,"Hashflow: исходный хеш, отправитель, поля RFQT или канонический блок не подтверждены")
        };
        let chain = ctx.chain_id().ok_or_else(bad)?;
        let t: Option<Transaction> = rpc::call(ctx, "eth_getTransactionByHash", json!([hash]))
            .await
            .map_err(|_| bad())?;
        if !t
            .as_ref()
            .is_some_and(|t| self.original_matches(t, chain, hash, nonce) && t.position(hash, r))
            || (r.status != U64::ZERO && (r.status != U64::from(1) || !self.matches(r)))
        {
            return Err(bad());
        }
        canonical_receipt::verify(ctx, hash, r)
            .await
            .map_err(|_| bad())
    }
    pub(super) async fn cancellation(
        &self,
        ctx: &Ctx,
        original: B256,
        cancel: B256,
        nonce: u64,
    ) -> GatewayResult<()> {
        let bad = || {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "Hashflow: отмена или исходная сеть не подтверждена по хешу и nonce",
            )
        };
        let chain = ctx.chain_id().ok_or_else(bad)?;
        if cancel == original {
            return Err(bad());
        }
        if let Some(saved) = self.original {
            if saved.chain != chain {
                return Err(bad());
            }
        } else {
            // Одна подтверждённая отмена в переназначенной сети не устанавливает сеть старой заявки.
            let t: Option<Transaction> =
                rpc::call(ctx, "eth_getTransactionByHash", json!([original]))
                    .await
                    .map_err(|_| bad())?;
            if !t.is_some_and(|t| self.original_matches(&t, chain, original, nonce)) {
                return Err(bad());
            }
        }
        let (t, r): (Option<Transaction>, Option<Receipt>) = tokio::try_join!(
            rpc::call(ctx, "eth_getTransactionByHash", json!([cancel])),
            super::super::super::send::receipt::read(ctx, cancel)
        )
        .map_err(|_| bad())?;
        let r = r.ok_or_else(bad)?;
        if !t.is_some_and(|t| {
            t.cancelled(chain, cancel, self.trader, nonce) && t.position(cancel, &r)
        }) {
            return Err(bad());
        }
        // Включённая транзакция с тем же nonce его занимает независимо от статуса EVM.
        canonical_receipt::verify(ctx, cancel, &r)
            .await
            .map_err(|_| bad())
    }
}

pub(super) fn same_fill(tx: &crate::core::types::ChainTx, r: &Receipt) -> bool {
    tx.outcome == crate::core::types::TxOutcome::Included
        && r.status == U64::from(1)
        && tx.block == Some(r.block_number.to())
        && tx.gas_used == Some(r.gas_used.to())
        && tx.gas_bnb == r.gas_bnb()
}

#[cfg(test)]
#[path = "proof_chain_tests.rs"]
mod tests;
