//! Файл: venues/bsc_dex/send/canonical_receipt.rs.
//! Денежный круг связывает расход/зачисление с транзакцией в каноническом блоке.
//! Повторная проверка перед POST использует тот же хеш блока, индекс и настроенную глубину.
//! Формат Ethereum Execution APIs: eth_getTransactionReceipt и eth_getBlockByNumber(false).
use super::{
    super::{ctx::Ctx, node::rpc},
    receipt::Receipt,
};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    settlement_proofs,
};
use alloy_primitives::{B256, U64};
use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Copy)]
struct Inclusion {
    chain: u64,
    tx: B256,
    number: U64,
    block: B256,
    index: usize,
}
#[derive(Deserialize)]
struct Block {
    number: U64,
    hash: B256,
    transactions: Vec<Transaction>,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Transaction {
    Hash(B256),
    Object { hash: B256 },
}
impl Transaction {
    fn hash(&self) -> B256 {
        match self {
            Self::Hash(v) | Self::Object { hash: v } => *v,
        }
    }
}
impl Inclusion {
    fn from(ctx: &Ctx, tx: B256, r: &Receipt) -> Option<Self> {
        if r.transaction_hash != Some(tx)
            || r.logs.iter().any(|l| {
                l.removed
                    || l.transaction_hash.is_some_and(|h| h != tx)
                    || l.block_hash.is_some_and(|h| Some(h) != r.block_hash)
                    || l.block_number
                        .is_some_and(|n| n != r.block_number.to::<u64>())
            })
        {
            return None;
        }
        Some(Self {
            tx,
            chain: ctx.chain_id()?,
            number: r.block_number,
            block: r.block_hash?,
            index: usize::try_from(r.transaction_index?.to::<u64>()).ok()?,
        })
    }
    fn matches(self, head: U64, depth: u64, block: &Block) -> bool {
        head.to::<u64>()
            .checked_sub(self.number.to())
            .is_some_and(|n| n >= depth)
            && block.number == self.number
            && block.hash == self.block
            && block
                .transactions
                .get(self.index)
                .is_some_and(|v| v.hash() == self.tx)
    }
    async fn verify(self, ctx: &Ctx) -> GatewayResult<()> {
        let depth = ctx.int("send.deposit_confirmations")?;
        let (network, head): (U64, U64) = tokio::try_join!(
            rpc::call(ctx, "eth_chainId", json!([])),
            rpc::call(ctx, "eth_blockNumber", json!([]))
        )?;
        let block: Option<Block> =
            rpc::call(ctx, "eth_getBlockByNumber", json!([self.number, false])).await?;
        if network.to::<u64>() == self.chain
            && ctx.chain_id() == Some(self.chain)
            && block.is_some_and(|b| self.matches(head, depth, &b))
        {
            Ok(())
        } else {
            Err(ctx.err(
                ErrorCode::OutcomeUnknown,
                format!(
                    "Транзакция {} ещё не подтверждена в исходном блоке {} с глубиной {}",
                    self.tx, self.block, depth
                ),
            ))
        }
    }
}
pub async fn observe(ctx: &Ctx, tx: B256, r: &Receipt) -> GatewayResult<()> {
    if !settlement_proofs::active() {
        return Ok(());
    }
    let inclusion=Inclusion::from(ctx,tx,r).ok_or_else(||ctx.err(ErrorCode::OutcomeUnknown,format!("У квитанции {tx} отсутствует однозначная привязка к хешу блока и индексу транзакции")))?;
    inclusion.verify(ctx).await?;
    let owner = ctx.clone();
    settlement_proofs::record(
        &ctx.venue(),
        &tx.to_string(),
        inclusion.block.to_string(),
        move || {
            let owner = owner.clone();
            Box::pin(async move { inclusion.verify(&owner).await })
        },
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replaced_same_height_and_changed_position_reject_both_rpc_shapes() {
        let rows: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../../../tests/data/hashflow_direct_2026-11-26/swaps.json"
        ))
        .unwrap();
        for row in rows {
            let mut raw = row["receipt"].clone();
            raw["transactionHash"] = row["transaction_hash"].clone();
            raw["blockHash"] = row["block_hash"].clone();
            let r: Receipt = serde_json::from_value(raw).unwrap();
            let tx = r.transaction_hash.unwrap();
            let index = r.transaction_index.unwrap().to::<usize>();
            let p = Inclusion {
                chain: row["chain_id"].as_u64().unwrap(),
                tx,
                index,
                number: r.block_number,
                block: r.block_hash.unwrap(),
            };
            for value in [json!(tx), json!({"hash":tx})] {
                let mut list = vec![json!(B256::ZERO); index + 1];
                list[index] = value;
                let mut b: Block = serde_json::from_value(
                    json!({"hash":p.block,"number":p.number,"transactions":list}),
                )
                .unwrap();
                let head = p.number + U64::from(2);
                assert!(p.matches(head, 2, &b));
                assert!(!p.matches(head, 3, &b));
                b.hash = B256::ZERO;
                assert!(!p.matches(head, 2, &b));
                b.hash = p.block;
                b.transactions[index] = Transaction::Hash(B256::ZERO);
                assert!(!p.matches(head, 2, &b));
            }
        }
    }
}
