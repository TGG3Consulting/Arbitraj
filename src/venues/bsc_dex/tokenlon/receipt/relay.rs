//! Сверка уже выплаченной личной заявки, доставленной внешним плательщиком газа.
//! Исходные Proof и digest берутся из durable intent владельца, записанного до разглашения подписи.
use super::{FillOrder, Proof, inclusion::Sender};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{ctx::Ctx, node::rpc, send::track::Receipt};
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, Bytes, U64, U256};
use serde::Deserialize;
use serde_json::json;

impl Proof {
    /// Все четыре топика FillOrder. Область поиска ограничивается PMM и блоками зарегистрированного поручения.
    pub(in crate::venues::bsc_dex) fn fill_topics(&self) -> [B256; 4] {
        [
            FillOrder::SIGNATURE_HASH,
            self.tx_hash,
            self.order_hash,
            self.terms.payer.into_word(),
        ]
    }
    /// Подтверждает обе суммы для собственного ERC20-плательщика. Ethereum nonce и газ относятся к ретранслятору.
    pub(in crate::venues::bsc_dex) async fn relayed(
        &self,
        ctx: &Ctx,
        hash: B256,
        calldata_digest: B256,
        r: &Receipt,
    ) -> GatewayResult<(U256, U256)> {
        let bad = || {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "Tokenlon: не подтверждено исполнение личной заявки ретранслятором",
            )
        };
        if r.status != U64::from(1) {
            return Err(bad());
        }
        let parent = self
            .check_delivery(ctx, hash, Sender::Relay, calldata_digest, r)
            .await?;
        if self.terms.output == self.terms.roles.weth.address {
            plain_receiver(ctx, self.terms.payer, parent, r.block_hash.ok_or_else(bad)?).await?;
        }
        self.bound_amounts(ctx, hash, r, None).await
    }
}
/// При чужой доставке nonce получателя ETH может оставаться прежним. Изменения nonce допустимы лишь
/// в количестве его обычных собственных транзакций; дополнительная авторизация кода требует ручной сверки.
async fn plain_receiver(ctx: &Ctx, payer: Address, parent: B256, block: B256) -> GatewayResult<()> {
    let tag = |h| json!({"blockHash":h,"requireCanonical":true});
    let bad = || {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "Tokenlon: получение нативного ETH исходным EOA требует сверки",
        )
    };
    let (code0, code1, before, after): (Bytes, Bytes, U64, U64) = tokio::try_join!(
        rpc::call(ctx, "eth_getCode", json!([payer, tag(parent)])),
        rpc::call(ctx, "eth_getCode", json!([payer, tag(block)])),
        rpc::call(ctx, "eth_getTransactionCount", json!([payer, tag(parent)])),
        rpc::call(ctx, "eth_getTransactionCount", json!([payer, tag(block)]))
    )
    .map_err(|_| bad())?;
    if !code0.is_empty() || !code1.is_empty() {
        return Err(bad());
    }
    let delta = after.to::<u64>().checked_sub(before.to()).ok_or_else(bad)?;
    if delta == 0 {
        return Ok(());
    }
    #[derive(Deserialize)]
    struct Transaction {
        from: Address,
    }
    #[derive(Deserialize)]
    struct Header {
        hash: B256,
        transactions: Vec<Transaction>,
    }
    let h: Option<Header> = rpc::call(ctx, "eth_getBlockByHash", json!([block, true]))
        .await
        .map_err(|_| bad())?;
    if h.is_some_and(|b| {
        b.hash == block
            && u64::try_from(b.transactions.iter().filter(|tx| tx.from == payer).count()).ok()
                == Some(delta)
    }) {
        Ok(())
    } else {
        Err(bad())
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
