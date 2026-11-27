//! Сверка полной подписанной операции с включённым хешем и блоком квитанции.
use super::super::{check::simulate::SimLog, ctx::Ctx, node::rpc, send::track::Receipt};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::rpc::types::Log;
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::Deserialize;
use serde_json::json;
pub(super) async fn included(
    ctx: &Ctx,
    hash: B256,
    r: &Receipt,
    payer: Address,
    target: Address,
    value: U256,
    input: Bytes,
) -> GatewayResult<()> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Tx {
        hash: B256,
        block_hash: Option<B256>,
        from: Address,
        to: Option<Address>,
        value: U256,
        input: Bytes,
    }
    let fail = || {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "Не подтверждены исходный хеш, блок или подписанное поручение кошелька",
        )
    };
    if r.transaction_hash != Some(hash) || r.block_hash.is_none() {
        return Err(fail());
    }
    let tx: Option<Tx> = rpc::call(ctx, "eth_getTransactionByHash", json!([hash]))
        .await
        .map_err(|_| fail())?;
    if tx.is_some_and(|t| {
        t.hash == hash
            && t.block_hash == r.block_hash
            && t.from == payer
            && t.to == Some(target)
            && t.value == value
            && t.input == input
    }) {
        Ok(())
    } else {
        Err(fail())
    }
}

pub(in crate::venues::bsc_dex) fn logs(rows: &[SimLog]) -> Option<Vec<Log>> {
    rows.iter()
        .map(|r| {
            Some(Log {
                inner: alloy_primitives::Log::new(r.address, r.topics.clone(), r.data.clone())?,
                ..Default::default()
            })
        })
        .collect()
}
