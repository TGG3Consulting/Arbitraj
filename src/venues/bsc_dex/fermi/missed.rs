//! Закрывает пропущенный слот только после проверки финальности, исходного поручения и всех состояний nonce.
use super::super::{exec, parts::Parts, resolve, send};
use super::{Head, execute::Marker, slot};
use crate::{
    core::types::{SendMethod, TradeResult, TxOutcome},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        pending::PendingRef,
        settlement_intent as intent,
    },
};
use alloy::primitives::{B256, U64};
use serde::Deserialize;
use serde_json::json;
fn free_nonce(nonce: u64, completed: bool, observations: [U64; 3]) -> bool {
    !completed && observations.iter().all(|seen| *seen == U64::from(nonce))
}
/// Только финальность и полное содержимое исходного канонического блока доказывают, что своп не состоялся.
/// Разрешение nonce требует одновременно свободного finalized/latest/pending. Ничто в этом пути не отправляется.
pub(super) async fn missed(
    p: &Parts,
    parse: &Marker,
    entry: &intent::Entry,
    hash: B256,
    nonce: u64,
    marker: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    #[derive(Deserialize)]
    struct Block {
        number: U64,
        hash: B256,
        transactions: Vec<B256>,
    }
    let ctx = &p.ctx;
    let o = &parse.order;
    let fin: Head =
        super::rpc::call(ctx, "eth_getBlockByNumber", json!(["finalized", false])).await?;
    if fin.number.to::<u64>() < o.target {
        return Ok(None);
    }
    let block: Block = super::rpc::call(
        ctx,
        "eth_getBlockByNumber",
        json!([U64::from(o.target), false]),
    )
    .await?;
    if block.number.to::<u64>() != o.target
        || block.hash.is_zero()
        || block.transactions.contains(&hash)
    {
        return Ok(None);
    }
    let tag = json!({"blockHash":fin.hash,"requireCanonical":true});
    let (completed, finalized, latest, pending): (bool, U64, U64, U64) = tokio::try_join!(
        super::canonical(
            ctx,
            fin.hash,
            o.executor,
            slot::completedCall {
                payer: o.payer,
                leg: o.leg
            }
        ),
        super::rpc::call(ctx, "eth_getTransactionCount", json!([o.payer, tag])),
        super::rpc::call(ctx, "eth_getTransactionCount", json!([o.payer, "latest"])),
        super::rpc::call(ctx, "eth_getTransactionCount", json!([o.payer, "pending"]))
    )?;
    if !free_nonce(nonce, completed, [finalized, latest, pending]) {
        return Ok(None);
    }
    let known: Option<serde_json::Value> =
        super::rpc::call(ctx, "eth_getTransactionByHash", json!([hash])).await?;
    if known.is_some() {
        return Ok(None);
    }
    resolve::release_if_ours(p, hash, Some(nonce), true).await;
    intent::archive_attempt(ctx.journal.db(),&parse.key,entry,"Финализированный целевой блок без исходной подписи, поручение не выполнено и nonce свободен")
        .await.map_err(|_|ctx.err(ErrorCode::OutcomeUnknown,"Не удалось сохранить сверенный исход Fermi").pending(marker.clone()))?;

    let fact = exec::chain_tx::chain_tx(
        hash,
        nonce,
        vec![o.builder.clone()],
        SendMethod::Bundle,
        &send::track::Tracked::bare(TxOutcome::NotIncluded, None),
        None,
    );
    Err(ctx
        .err(
            ErrorCode::TxNotIncluded,
            "Целевой блок Fermi финализирован без этой транзакции, поручение не исполнено",
        )
        .with_tx(fact))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absence_never_releases_completed_or_disputed_nonce() {
        let n = U64::from(7);
        assert!(free_nonce(7, false, [n; 3]));
        assert!(!free_nonce(7, true, [n; 3]));
        for pos in 0..3 {
            for other in [6u64, 8] {
                let mut samples = [n; 3];
                samples[pos] = U64::from(other);
                assert!(!free_nonce(7, false, samples));
            }
        }
    }
}
