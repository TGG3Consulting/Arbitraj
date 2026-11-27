//! Сверка полной подписанной операции с включённым хешем и блоком квитанции.
use super::super::{check::simulate::SimLog, ctx::Ctx, node::rpc, send::track::Receipt};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::rpc::types::Log;
use alloy_primitives::{Address, B256, Bytes, U64,U256,keccak256};
use serde::{Serialize,Deserialize};
use serde_json::json;
/// Old issuer, Shiba and Empire records were written exclusively on Ethereum before
/// their serialized orders included the EIP-155 chain identifier.
pub(super) fn legacy_ethereum_chain() -> u64 {crate::settings::evm_profiles::ETHEREUM_MAINNET}

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

/// Полный исходный хеш calldata позволяет сверить подпись заявки без сохранения её в журнале.
#[derive(Clone,Copy,Debug,Serialize,Deserialize)]
pub(crate) struct Erc20Call {pub chain:u64,pub payer:Address,pub module:Address,pub calldata:B256}
impl Erc20Call {
    pub(in crate::venues::bsc_dex) async fn cancelled(mut self,ctx:&Ctx,original:B256,cancel:B256,nonce:u64,r:&Receipt)->GatewayResult<()> {
        if cancel==original {return Err(ctx.err(ErrorCode::OutcomeUnknown,"Один хеш заявлен для сделки и отмены"));}
        self.module=self.payer;
        self.calldata=keccak256([]);
        self.included(ctx,cancel,nonce,r).await
    }
    pub(in crate::venues::bsc_dex) async fn included(&self,ctx:&Ctx,hash:B256,nonce:u64,r:&Receipt)->GatewayResult<()> {
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
        let fail = || {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "Не подтверждено исходное поручение ERC20 в каноническом блоке",
            )
        };
        if !(ctx.chain_id()==Some(self.chain) && super::super::wallet::signer::address(ctx).ok()==Some(self.payer)) || r.transaction_hash != Some(hash) || r.block_hash.is_none() {
            return Err(fail());
        }
        let tx: Option<Tx> = rpc::call(ctx, "eth_getTransactionByHash", json!([hash]))
            .await
            .map_err(|_| fail())?;
        if !tx.is_some_and(|t| {
            t.hash == hash
                && Some(t.block_hash) == r.block_hash
                && Some(t.transaction_index) == r.transaction_index
                && t.chain_id.to::<u64>() == self.chain
                && t.from == self.payer
                && t.to == self.module
                && t.nonce.to::<u64>() == nonce
                && t.value.is_zero()
                && keccak256(&t.input) == self.calldata
        }) {
            return Err(fail());
        }
        super::super::send::canonical_receipt::verify(ctx, hash, r)
            .await
            .map_err(|_| fail())?;
        Ok(())
    }
}
