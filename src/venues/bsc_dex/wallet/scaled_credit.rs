//! Доказательство размера депозита в UI-единицах из двух событий ERC-8056 одной транзакции.
//! https://eips.ethereum.org/EIPS/eip-8056 . Текущий множитель не восстанавливает исторический перевод.

use std::collections::{HashMap, HashSet};

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, U64, U256};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::node::{abi::IERC20, rpc};
use super::super::{num, parts::Parts};
use super::{signer, transfer};

alloy::sol! {
    event TransferWithUIAmount(address indexed from, address indexed to, uint256 rawAmount, uint256 uiAmount);
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    transaction_hash: B256,
    block_hash: B256,
    block_number: U64,
    status: U64,
    logs: Vec<Log>,
}

/// Только для уже зачисленной суммы и её хеша. При отсутствии доказательства raw-кредит остаётся неизменным.
pub(in crate::venues::bsc_dex) async fn confirmed(
    p: &Parts,
    asset: &Asset,
    hash: &str,
    credited: Decimal,
    incoming: bool,
) -> GatewayResult<Option<(Decimal, Decimal)>> {
    let ctx = &p.ctx;
    let tx: B256 = hash.parse().map_err(|_| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("«{hash}» — не хеш зачисления"),
        )
    })?;
    let token = transfer::token(ctx, asset)?;
    let wallet = signer::address(ctx)?;
    let receipt: Option<Receipt> = rpc::call(ctx, "eth_getTransactionReceipt", json!([tx])).await?;
    let Some(receipt) = receipt else {
        return Ok(None);
    };
    let Some((raw, ui)) = directional_pairs(&receipt, tx, token, wallet, incoming) else {
        return Ok(None);
    };
    let (decimals, scale) = p
        .scaled_units
        .read(ctx, token, receipt.block_number.to())
        .await?;
    Ok(scale.and_then(|m| credited_ui(raw, ui, credited, decimals).map(|amount| (amount, m))))
}

fn credited_ui(raw: U256, ui: U256, credited: Decimal, decimals: u8) -> Option<Decimal> {
    let exact = |u| num::to_decimal(u, decimals).filter(|d| num::to_units(*d, decimals) == Some(u));
    (exact(raw)? == credited).then(|| exact(ui)).flatten()
}

/// Суммы — только по входящим переводам того же токена. Все они обязаны иметь пару с тем же
/// отправителем и raw. Частичное покрытие, расход из этого же кошелька и повтор лога не являются доказательством.
fn directional_pairs(
    receipt: &Receipt,
    tx: B256,
    token: Address,
    wallet: Address,
    incoming: bool,
) -> Option<(U256, U256)> {
    if receipt.transaction_hash != tx || receipt.status != U64::from(1u8) {
        return None;
    }
    let wallet_topic = B256::left_padding_from(wallet.as_slice());
    let mut indices = HashSet::new();
    let mut counts = HashMap::<(Address, U256), i64>::new();
    let mut beneficiary = None;
    let (mut raw, mut ui) = (U256::ZERO, U256::ZERO);
    for log in receipt.logs.iter().filter(|l| l.address() == token) {
        let topics = log.topics();
        let Some(&topic) = topics.first() else {
            continue;
        };
        if topic != IERC20::Transfer::SIGNATURE_HASH
            && topic != TransferWithUIAmount::SIGNATURE_HASH
        {
            continue;
        }
        let (own, other) = if incoming { (2, 1) } else { (1, 2) };
        if topics.get(other) == Some(&wallet_topic) {
            return None;
        }
        if topics.get(own) != Some(&wallet_topic) {
            continue;
        }
        if !incoming {
            let address = *topics.get(other)?;
            if beneficiary
                .replace(address)
                .is_some_and(|prev| prev != address)
            {
                return None;
            }
        }
        if topics.len() != 3
            || log.removed
            || log.transaction_hash != Some(tx)
            || log.block_hash != Some(receipt.block_hash)
            || log.block_number != Some(receipt.block_number.to())
            || !indices.insert(log.log_index?)
        {
            return None;
        }
        if topic == IERC20::Transfer::SIGNATURE_HASH {
            let t = IERC20::Transfer::decode_log_data_validate(&log.inner.data).ok()?;
            *counts.entry((t.from, t.value)).or_default() += 1;
            raw = raw.checked_add(t.value)?;
        } else {
            let t = TransferWithUIAmount::decode_log_data_validate(&log.inner.data).ok()?;
            *counts.entry((t.from, t.rawAmount)).or_default() -= 1;
            ui = ui.checked_add(t.uiAmount)?;
        }
    }
    (!raw.is_zero() && !ui.is_zero() && counts.values().all(|n| *n == 0)).then_some((raw, ui))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn pairs(r: &Receipt, tx: B256, token: Address, wallet: Address) -> Option<(U256, U256)> {
        directional_pairs(r, tx, token, wallet, true)
    }

    // Записанные данные BSC. Идентификаторы заменены согласованно; порядок и суммы событий сохранены.
    #[test]
    fn both_polo_withdrawals_match_wallet_and_credited_raw() {
        let token = Address::from_str("0x3ee4df61bd4f867e349beae8bfe07bc31b4850fb").unwrap();
        let wallet = Address::from_str("0x47f5b4462e13bb1d690e9e9ded9373b8b799079d").unwrap();
        let mut receipts: Vec<Receipt> = serde_json::from_str(include_str!(
            "../../../../tests/data/bsc_units_2026_11_24/deposit_receipts.json"
        ))
        .unwrap();
        let amounts = [
            ("0.40358684625", "3.22869477"),
            ("0.04552765", "0.36422120"),
        ];
        assert_eq!(receipts.len(), amounts.len());
        for (receipt, (raw, ui)) in receipts.iter().zip(amounts) {
            let tx = receipt.transaction_hash;
            let (a, b) = pairs(receipt, tx, token, wallet).unwrap();
            assert_eq!(
                credited_ui(a, b, raw.parse().unwrap(), 18),
                Some(ui.parse().unwrap())
            );
            assert_eq!(credited_ui(a, b, ui.parse().unwrap(), 18), None);
            assert_eq!(pairs(receipt, tx, token, Address::ZERO), None);
            assert_eq!(pairs(receipt, B256::ZERO, token, wallet), None);
        }
        let first = &mut receipts[0];
        first
            .logs
            .retain(|l| l.topics().first() != Some(&TransferWithUIAmount::SIGNATURE_HASH));
        assert_eq!(pairs(first, first.transaction_hash, token, wallet), None);
    }
}
