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
pub(super) struct Tx {
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

#[derive(Clone, Copy)]
pub(super) enum Sender {
    Owner(u64),
    Relay,
}

impl Tx {
    pub(super) fn matches(&self, proof: &Proof, sender: Sender, digest: B256, at: u64) -> bool {
        let authorized = match sender {
            Sender::Owner(nonce) => {
                self.from == proof.terms.payer && self.nonce == U64::from(nonce)
            }
            Sender::Relay => {
                !self.from.is_zero() && self.from != proof.terms.payer && proof.terms.input != WETH
            }
        };
        authorized
            && proof.terms.recipient == proof.terms.payer
            && self.chain_id == U64::from(crate::settings::evm_profiles::ETHEREUM_MAINNET)
            && self.to == USER_PROXY
            && self.value == proof.value()
            && keccak256(&self.input) == digest
            && Prepared::from_calldata(&self.input, &proof.terms, at)
                .is_some_and(|p| p.proof == *proof)
    }
}

impl Proof {
    /// hash и nonce получены из исходной отправки, calldata_digest сохранён до неё. Отказ не разрешает повтор.
    pub(in crate::venues::bsc_dex) async fn transaction_matches(
        &self,
        ctx: &Ctx,
        hash: B256,
        nonce: u64,
        calldata_digest: B256,
        r: &Receipt,
    ) -> GatewayResult<()> {
        self.check_delivery(ctx, hash, Sender::Owner(nonce), calldata_digest, r)
            .await
            .map(|_| ())
    }
    pub(super) async fn check_delivery(
        &self,
        ctx: &Ctx,
        hash: B256,
        sender: Sender,
        calldata_digest: B256,
        r: &Receipt,
    ) -> GatewayResult<B256> {
        let fail = || {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "Tokenlon: не подтверждены исходная операция и обе выплаты PMM",
            )
        };
        if ctx.chain_id() != Some(crate::settings::evm_profiles::ETHEREUM_MAINNET)
            || self.terms.recipient != self.terms.payer
            || signer::address(ctx).ok() != Some(self.terms.payer)
            || r.status > U64::from(1)
            || r.transaction_hash != Some(hash)
        {
            return Err(fail());
        }
        canonical_receipt::verify(ctx, hash, r)
            .await
            .map_err(|_| fail())?;
        let block = r.block_hash.ok_or_else(fail)?;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Header {
            hash: B256,
            parent_hash: B256,
            timestamp: U64,
        }
        let (tx, header): (Option<Tx>, Option<Header>) = tokio::try_join!(
            rpc::call(ctx, "eth_getTransactionByHash", json!([hash])),
            rpc::call(ctx, "eth_getBlockByHash", json!([block, false]))
        )
        .map_err(|_| fail())?;
        let tx = tx.ok_or_else(fail)?;
        // Истечение до включения может быть причиной status=0. Подлинность подписи и хеша сохраняется.
        let header = header.ok_or_else(fail)?;
        let timestamp = header.timestamp.to();
        let at = if r.status.is_zero() { 0 } else { timestamp };
        if tx.hash != hash
            || tx.block_hash != block
            || Some(tx.transaction_index) != r.transaction_index
            || header.hash != block
            || !tx.matches(self, sender, calldata_digest, at)
        {
            return Err(fail());
        }
        Ok(header.parent_hash)
    }

    /// Проверка исполнения status=1. При восстановлении этого же хеша используются тот же фильтр и роли.
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
                "Tokenlon: не подтверждена собственная выплата PMM",
            )
        };
        if r.status != U64::from(1) {
            return Err(fail());
        }
        self.transaction_matches(ctx, hash, nonce, calldata_digest, r)
            .await?;
        self.bound_amounts(
            ctx,
            hash,
            r,
            (self.terms.output == WETH).then_some(self.terms.recipient),
        )
        .await
    }
    pub(super) async fn bound_amounts(
        &self,
        ctx: &Ctx,
        hash: B256,
        r: &Receipt,
        own_native: Option<Address>,
    ) -> GatewayResult<(U256, U256)> {
        let fail = || {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "Tokenlon: исходная квитанция не подтверждает направления и суммы",
            )
        };
        let code = Code {
            shell: b256!("abf73cab47631c9363fa11b421585b9edc25458176a4e00292846ed8a45b3c1a"),
            implementation: address!("0b9f13ffab8448089f50073cf24bbe5c7bd8675a"),
            runtime: b256!("db3bf7e4d3bea9449bdaaf049140386c9c0d57c1d9fcb962a88310b002fc70a5"),
        };
        code.at_transaction(ctx, USER_PROXY, hash, r, own_native)
            .await?;
        self.amounts(&r.logs).ok_or_else(fail)
    }
}
