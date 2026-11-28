//! Привязка денежного результата к исходной собственной транзакции и её месту в каноническом блоке.
use super::super::wire::{Prepared, USER_PROXY};
use super::{Proof, WETH};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{
    ctx::Ctx,
    exec::carbon_binding::Code,
    node::rpc,
    send::{canonical_receipt, track::Receipt},
    wallet::signer,
};
use alloy_primitives::{Address, B256, Bytes, U64, U256, address, b256, keccak256};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tx {
    hash: B256,
    block_hash: B256,
    transaction_index: U64,
    chain_id: U64,
    from: Address,
    to: Address,
    nonce: U64,
    input: Bytes,
    value: U256,
}

impl Proof {
    /// hash и nonce получены из исходной отправки, calldata_digest сохранён до неё. Отказ не разрешает повтор.
    pub(in crate::venues::bsc_dex) async fn included(
        &self,
        ctx: &Ctx,
        hash: B256,
        nonce: u64,
        calldata_digest: B256,
        r: &Receipt,
    ) -> GatewayResult<(U256, U256)> {
        let fail = || {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "Tokenlon: не подтверждены исходная операция и обе выплаты PMM",
            )
        };
        if ctx.chain_id() != Some(crate::settings::evm_profiles::ETHEREUM_MAINNET)
            || self.terms.recipient != self.terms.payer
            || signer::address(ctx).ok() != Some(self.terms.payer)
            || r.status != U64::from(1)
            || r.transaction_hash != Some(hash)
        {
            return Err(fail());
        }
        canonical_receipt::verify(ctx, hash, r)
            .await
            .map_err(|_| fail())?;
        let block = r.block_hash.ok_or_else(fail)?;
        #[derive(Deserialize)]
        struct Header {
            timestamp: U64,
        }
        let (tx, header): (Option<Tx>, Option<Header>) = tokio::try_join!(
            rpc::call(ctx, "eth_getTransactionByHash", json!([hash])),
            rpc::call(ctx, "eth_getBlockByHash", json!([block, false]))
        )
        .map_err(|_| fail())?;
        let tx = tx.ok_or_else(fail)?;
        if tx.hash != hash
            || tx.block_hash != block
            || Some(tx.transaction_index) != r.transaction_index
            || tx.chain_id != U64::from(crate::settings::evm_profiles::ETHEREUM_MAINNET)
            || tx.from != self.terms.payer
            || tx.to != USER_PROXY
            || tx.nonce != U64::from(nonce)
            || tx.value != self.value()
            || keccak256(&tx.input) != calldata_digest
            || Prepared::from_calldata(
                &tx.input,
                &self.terms,
                header.ok_or_else(fail)?.timestamp.to(),
            )
            .is_none_or(|p| p.proof != *self)
        {
            return Err(fail());
        }
        let code = Code {
            shell: b256!("abf73cab47631c9363fa11b421585b9edc25458176a4e00292846ed8a45b3c1a"),
            implementation: address!("0b9f13ffab8448089f50073cf24bbe5c7bd8675a"),
            runtime: b256!("db3bf7e4d3bea9449bdaaf049140386c9c0d57c1d9fcb962a88310b002fc70a5"),
        };
        code.at_transaction(
            ctx,
            USER_PROXY,
            hash,
            r,
            (self.terms.output == WETH).then_some(self.terms.recipient),
        )
        .await?;
        self.amounts(&r.logs).ok_or_else(fail)
    }
}
