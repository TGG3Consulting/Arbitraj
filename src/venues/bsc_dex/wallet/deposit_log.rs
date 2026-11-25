//! Индивидуальное зачисление ERC20 из пакетной выплаты. Привязка записывается
//! до выдачи Credited и переживает повтор опроса, смену процесса и гонку двух выводов.

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, U256};
use rusqlite::{OptionalExtension, params};

use super::super::{ctx::Ctx, node::abi::IERC20, send::track::Receipt};
use crate::gateway::{
    contract::DepositClaim,
    errors::{ErrorCode, GatewayResult},
};

pub(super) fn incoming(
    receipt: &Receipt,
    token: Address,
    wallet: Address,
    hash: B256,
) -> Result<Vec<(u64, U256)>, &'static str> {
    let mut incoming = Vec::new();
    for log in receipt.logs.iter().filter(|l| l.address() == token) {
        if log.topics().len() != 3 || log.topics()[0] != IERC20::Transfer::SIGNATURE_HASH {
            continue;
        }
        let transfer = IERC20::Transfer::decode_log_data(&log.inner.data)
            .map_err(|_| "Повреждён ERC-20 Transfer в квитанции")?;
        if transfer.to != wallet || transfer.from == wallet || transfer.value.is_zero() {
            continue;
        }
        if log.removed
            || log.transaction_hash != Some(hash)
            || log.block_number != Some(receipt.block_number.to())
        {
            return Err("Лог зачисления не подтверждён этой квитанцией");
        }
        let index = log
            .log_index
            .ok_or("В логе зачисления отсутствует logIndex")?;
        if incoming.iter().any(|(seen, _)| *seen == index) {
            return Err("Квитанция содержит повторный logIndex");
        }
        incoming.push((index, transfer.value));
    }
    Ok(incoming)
}

pub(super) fn select(
    logs: &[(u64, U256)],
    expected: Option<U256>,
) -> Result<(u64, U256), &'static str> {
    if let [one] = logs {
        return Ok(*one);
    }
    if logs.is_empty() {
        return Err("В квитанции нет поступления этого токена на кошелёк");
    }
    let mut matching = logs
        .iter()
        .copied()
        .filter(|(_, value)| Some(*value) == expected);
    match (matching.next(), matching.next()) {
        (Some(one), None) => Ok(one),
        _ => Err("Пакетный вывод: индивидуальное поступление не определено, нужна сверка"),
    }
}

/// Если после рестарта справка отправителя отсутствует, уже подтверждённый индекс
/// можно переиспользовать только при совпадении всех ключей и фактической суммы.
pub(super) async fn previous(
    ctx: &Ctx,
    hash: B256,
    token: Address,
    wallet: Address,
    claim: &DepositClaim,
    logs: &[(u64, U256)],
) -> GatewayResult<Option<(u64, U256)>> {
    let chain = ctx
        .chain_id()
        .ok_or_else(|| ctx.err(ErrorCode::NotConfigured, "Не задана сеть для зачисления"))?;
    let identity = [
        chain.to_string(),
        token.to_string(),
        wallet.to_string(),
        hash.to_string(),
    ];
    let claim = claim.clone();
    let saved = ctx
        .journal
        .db()
        .ledger()
        .read(move |c| find(c, &identity, &claim))
        .await
        .map_err(|e| {
            ctx.err(
                ErrorCode::Internal,
                format!("Чтение привязки ERC-20 зачисления: {e}"),
            )
        })?;
    Ok(saved.and_then(|(index, raw)| {
        logs.iter()
            .copied()
            .find(|(i, n)| *i == index && n.to_string() == raw)
    }))
}

fn find(
    c: &rusqlite::Connection,
    [chain, token, wallet, hash]: &[String; 4],
    owner: &DepositClaim,
) -> rusqlite::Result<Option<(u64, String)>> {
    c.query_row(
        "SELECT log_index,raw_amount FROM deposit_log_claim WHERE chain=?1 AND token=?2
          AND wallet=?3 AND tx=?4 AND sender=?5 AND withdrawal_id=?6 AND deal=?7",
        params![
            chain,
            token,
            wallet,
            hash,
            owner.sender.0,
            owner.withdrawal_id,
            owner.deal
        ],
        |row| {
            let i: i64 = row.get(0)?;
            Ok((
                u64::try_from(i).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, i))?,
                row.get(1)?,
            ))
        },
    )
    .optional()
}

pub(super) async fn bind(
    ctx: &Ctx,
    hash: B256,
    token: Address,
    wallet: Address,
    index: u64,
    raw: U256,
    claim: &DepositClaim,
) -> GatewayResult<()> {
    let chain = ctx.chain_id().ok_or_else(|| {
        ctx.err(
            ErrorCode::NotConfigured,
            "Не задан dex.evm_chain_id для зачисления",
        )
    })?;
    if claim.withdrawal_id.is_empty() || claim.deal.is_empty() {
        return Err(ctx.err(
            ErrorCode::DepositNotCredited,
            "Нет идентификатора вывода для привязки зачисления",
        ));
    }
    let claim = claim.clone();
    let identity = [
        chain.to_string(),
        token.to_string(),
        wallet.to_string(),
        hash.to_string(),
    ];
    let owned = ctx
        .journal
        .db()
        .ledger()
        .write(move |c| reserve(c, &identity, index, &raw.to_string(), &claim))
        .await
        .map_err(|e| {
            ctx.err(
                ErrorCode::Internal,
                format!("Привязка ERC-20 зачисления в учёте: {e}"),
            )
        })?;
    if !owned {
        return Err(ctx.err(
            ErrorCode::DepositNotCredited,
            "Лог зачисления или вывод уже закреплён за другой записью; нужна сверка",
        ));
    }
    Ok(())
}

fn reserve(
    c: &mut rusqlite::Connection,
    [chain, token, wallet, hash]: &[String; 4],
    index: u64,
    raw: &str,
    owner: &DepositClaim,
) -> rusqlite::Result<bool> {
    let index =
        i64::try_from(index).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let tx = c.transaction()?;
    tx.execute(
        "INSERT INTO deposit_log_claim(chain,token,wallet,tx,log_index,sender,withdrawal_id,deal,raw_amount)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT DO NOTHING",
        params![chain,token,wallet,hash,index,owner.sender.0,owner.withdrawal_id,owner.deal,raw],
    )?;
    let same = tx.query_row(
        "SELECT 1 FROM deposit_log_claim WHERE chain=?1 AND token=?2 AND wallet=?3 AND tx=?4 AND log_index=?5
         AND sender=?6 AND withdrawal_id=?7 AND deal=?8 AND raw_amount=?9",
        params![chain,token,wallet,hash,index,owner.sender.0,owner.withdrawal_id,owner.deal,raw], |_| Ok(()),
    ).optional()?.is_some();
    tx.commit()?;
    Ok(same)
}
#[cfg(test)]
#[path = "deposit_log_tests.rs"]
mod tests;
