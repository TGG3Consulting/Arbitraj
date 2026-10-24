//! Файл: venues/bsc_dex/wallet/native_status.rs
//! Что делает: нативный перевод BNB в статусах вывода и зачисления (решение 62). У нативного перевода нет события
//! Transfer — сумма и получатель только в самой транзакции (eth_getTransactionByHash: from, to, value, input):
//! - вывод с кошелька на биржу (wallet/transfer.rs) — транзакция от кошелька, value > 0, input пуст;
//! - зачисление с биржи — транзакция на адрес кошелька (to) с value > 0. Если биржа отправила BNB вызовом своего
//!   контракта (внутренний перевод), в транзакции этого не видно — честная ошибка BadResponse с причиной, без догадки.
//!   Настоящий вывод BNB с BingX (2026-10-24, tests/data/native_2026-10-25) — прямой перевод: input пуст, value = сумма
//!   вывода за вычетом комиссии, событий нет.
//!
//! Кто вызывает: wallet/transfer_status.rs.
//! От чего зависит: node/rpc.rs, exec/native.rs, ctx.rs.
//! Настройки: через exec/native.rs — dex.native_ticker, dex.native_wrapped [bsc_dex].
//!
//! eth_getTransactionByHash → TransactionInfo (from, to, value, input) — execution-apis 16caee27
//! src/eth/transaction.yaml (eth_getTransactionByHash), src/schemas/transaction.yaml (TransactionInfo, GenericTransaction).

use alloy_primitives::{Address, B256, Bytes, U256};
use serde::Deserialize;
use serde_json::json;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::exec::native;
use super::super::node::rpc;

/// Нужные поля транзакции (TransactionInfo).
#[derive(Debug, Clone, Deserialize)]
pub struct Tx {
    pub from: Address,
    /// null — создание контракта.
    pub to: Option<Address>,
    pub value: U256,
    pub input: Bytes,
}

/// Нативный ли это перевод и на сколько. Принимает: хеш, кошелёк, монету депозита (None — вывод), был ли в квитанции
/// перевод токена кошелька (у вывода). Возвращает: Some((монета, wei)) — нативный перевод; None — обычный токен.
/// Ошибки: узел; BadResponse — транзакции нет у узла или зачисление BNB не видно в транзакции (не на кошелёк).
pub async fn check(
    ctx: &Ctx,
    h: B256,
    wallet: Address,
    deposit: Option<&Asset>,
    token_transfer: bool,
) -> GatewayResult<Option<(Asset, U256)>> {
    let asset = match deposit {
        Some(a) if native::is_native(ctx, a) => a.clone(),
        Some(_) => return Ok(None),
        None if token_transfer => return Ok(None),
        None => match native::wrapped(ctx) {
            Some((ticker, w)) => Asset {
                ticker,
                bsc_contract: Some(w),
            },
            None => return Ok(None),
        },
    };
    let tx: Option<Tx> = rpc::call(ctx, "eth_getTransactionByHash", json!([h])).await?;
    let tx = tx.ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Транзакция {h}: квитанция есть, а самой транзакции у узла нет"),
        )
    })?;
    judge(&tx, wallet, asset, deposit.is_some())
        .map_err(|why| ctx.err(ErrorCode::BadResponse, format!("Транзакция {h}: {why}")))
}

/// Разбор транзакции нативной монеты. Вывод (deposit = false): свой нативный перевод — от кошелька, value > 0, без
/// вызова (input пуст), иначе None (не наш перевод BNB). Зачисление: транзакция на кошелёк с value > 0, иначе ошибка —
/// возможно, биржа перевела вызовом своего контракта (внутренний перевод в транзакции не виден), нужна ручная проверка.
/// Принимает: транзакцию, кошелёк, монету, зачисление ли. Возвращает: Some((монета, wei)) или None. Ошибки: текст.
pub fn judge(
    tx: &Tx,
    wallet: Address,
    asset: Asset,
    deposit: bool,
) -> Result<Option<(Asset, U256)>, String> {
    if !deposit {
        let own = tx.from == wallet && !tx.value.is_zero() && tx.input.is_empty();
        return Ok(own.then_some((asset, tx.value)));
    }
    if tx.to != Some(wallet) || tx.value.is_zero() {
        return Err(format!(
            "зачисления {} на кошелёк не видно (to {:?}, value {}) — возможно, биржа перевела вызовом своего контракта; нужна ручная проверка",
            asset.ticker, tx.to, tx.value
        ));
    }
    Ok(Some((asset, tx.value)))
}
