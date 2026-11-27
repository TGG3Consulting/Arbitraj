//! Поля исходной транзакции и канонического блока для восстановления Fermi и PropAMM.
//! Сверка внешнего денежного шага привязывает сохранённый ключ к кошельку и подписанной ноге.
use crate::{gateway::{errors::{ErrorCode, GatewayError}, pending::PendingRef}, venues::bsc_dex::{parts::Parts, send}};
use alloy::primitives::{Address, B256, Bytes, U64, U256};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Inclusion {
    pub(super) from: Address,
    pub(super) to: Address,
    #[serde(flatten)]
    pub(super) receipt: send::receipt::Receipt,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Header {
    pub(super) hash: B256,
    pub(super) parent_hash: B256,
    pub(super) timestamp: U64,
    pub(super) number: U64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Original {
    pub(super) hash: B256,
    pub(super) block_hash: B256,
    pub(super) from: Address,
    pub(super) to: Address,
    pub(super) nonce: U64,
    pub(super) input: Bytes,
    pub(super) value: U256,
}

pub(super) fn outer_matches(
    account: &str,
    bound: Option<&str>,
    key: &str,
    active: Option<&str>,
    leg: B256,
) -> bool {
    !account.is_empty()
        && !key.is_empty()
        && bound == Some(account)
        && active.is_none_or(|value| value == key)
        && alloy_primitives::keccak256(key.as_bytes()) == leg
}

/// Возвращает ошибку ручной сверки с реквизитами первоначальной передачи.
pub(super) fn unknown(p: &Parts, marker: &PendingRef) -> GatewayError {
    p.ctx.err(ErrorCode::OutcomeUnknown, "Fermi: исход по сохранённой подписи ещё не подтверждён")
        .pending(marker.clone())
}
