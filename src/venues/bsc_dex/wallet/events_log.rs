//! Файл: venues/bsc_dex/wallet/events_log.rs
//! Что делает: разбор события Transfer токена, касающегося кошелька бота (решение 76): фильтры подписки logs «на
//! кошелёк» и «с кошелька», направление и сумма, ключ лога (хеш транзакции + номер лога — для отмены при реорге).
//! Чистые функции — проверяются на записанных квитанциях блоков (tests/bsc_dex_wallet_events.rs).
//! Кто вызывает: wallet/events.rs; тесты.
//! От чего зависит: node/abi.rs (событие Transfer), alloy (Log).
//! Настройки: нет.
//!
//! Transfer(address indexed from, address indexed to, uint256 value) — https://eips.ethereum.org/EIPS/eip-20 :
//! topics = [topic0, from, to], value — в data. У ERC-721 то же имя с tokenId в четвёртом topic — такие логи не наши
//! (у них 4 topics) и пропускаются. Фильтр logs — execution-apis filter.yaml (позиции topics, null — любой).

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, U256};
use serde_json::{Value, json};

use super::super::node::abi::IERC20;

/// Фильтры подписки logs: Transfer любого токена на кошелёк и с кошелька. Ошибок нет.
pub fn filters(wallet: Address) -> [Value; 2] {
    let t0 = IERC20::Transfer::SIGNATURE_HASH;
    let w = B256::left_padding_from(wallet.as_slice());
    [
        json!({ "topics": [t0, Value::Null, w] }),
        json!({ "topics": [t0, w] }),
    ]
}

/// Перевод токена, касающийся кошелька.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub token: Address,
    /// true — на кошелёк, false — с кошелька.
    pub inbound: bool,
    /// Сумма в единицах токена.
    pub units: U256,
    pub tx: B256,
    pub block: u64,
    /// Ключ лога: (хеш транзакции, номер лога в блоке).
    pub key: (B256, u64),
    /// Лог отменён реоргом (removed: true).
    pub removed: bool,
}

/// Разбирает лог. Принимает: лог, кошелёк. Возвращает: Some(Move) — Transfer токена на кошелёк или с него (перевод
/// самому себе и нулевая сумма — None); None — не наш лог. Ошибки: текст — у лога нет хеша, блока или номера.
pub fn parse(log: &Log, wallet: Address) -> Result<Option<Move>, String> {
    if log.topics().len() != 3 || log.topics()[0] != IERC20::Transfer::SIGNATURE_HASH {
        return Ok(None);
    }
    let Ok(t) = IERC20::Transfer::decode_log_data(&log.inner.data) else {
        return Ok(None);
    };
    let inbound = match (t.from == wallet, t.to == wallet) {
        (false, true) => true,
        (true, false) => false,
        _ => return Ok(None),
    };
    if t.value.is_zero() {
        return Ok(None);
    }
    let tx = log
        .transaction_hash
        .ok_or("у лога Transfer нет transactionHash")?;
    let block = log.block_number.ok_or("у лога Transfer нет blockNumber")?;
    let index = log.log_index.ok_or("у лога Transfer нет logIndex")?;
    Ok(Some(Move {
        token: log.address(),
        inbound,
        units: t.value,
        tx,
        block,
        key: (tx, index),
        removed: log.removed,
    }))
}
