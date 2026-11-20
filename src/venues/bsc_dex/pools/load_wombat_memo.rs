//! Файл: venues/bsc_dex/pools/load_wombat_memo.rs
//! Что делает: решение 484 (ревью t253) — память неизменных частей пула Wombat: монеты (getTokens), их активы
//! (addressOfAsset) и decimals активов (underlyingTokenDecimals — immutable, Asset.sol L24). Они меняются только
//! добавлением / удалением актива (AssetAdded / AssetRemoved, PoolV2.sol L185-L219) — тогда запись забывается
//! (events_wombat.rs), как и при проверке пула (verify_wombat.rs). Есть запись — чтение каждым блоком берёт только
//! меняющиеся величины одной пачкой (load_wombat.rs); нет — полное чтение (три ступени со сверкой underlyingToken /
//! pool() активов) и запись.
//! Кто вызывает: pools/load_wombat.rs (get, put), pools/events_wombat.rs и pools/verify_wombat.rs (forget).
//! От чего зависит: std (Mutex, HashMap).
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use alloy_primitives::Address;

/// Неизменные части пула: монеты, активы и decimals по порядку getTokens().
#[derive(Debug, Clone, PartialEq)]
pub struct Fixed {
    pub tokens: Vec<Address>,
    pub assets: Vec<Address>,
    pub decimals: Vec<u8>,
}

/// Ключ — (id DEX, пул): один адрес пула в разных DEX / сетях не смешивается.
type Key = (String, Address);

static MEMO: LazyLock<Mutex<HashMap<Key, Fixed>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Запись пула. Принимает: id DEX, пул. Возвращает: копию или None. Ошибок нет (замок отравлен — None: полное чтение).
pub fn get(id: &str, pool: Address) -> Option<Fixed> {
    MEMO.lock().ok()?.get(&(id.to_string(), pool)).cloned()
}

/// Запомнить после полного чтения. Принимает: id DEX, пул, части. Ошибок нет (замок отравлен — не запоминается).
pub fn put(id: &str, pool: Address, fixed: Fixed) {
    if let Ok(mut m) = MEMO.lock() {
        m.insert((id.to_string(), pool), fixed);
    }
}

/// Забыть пул (AssetAdded / AssetRemoved, проверка пула). Принимает: id DEX, пул. Ошибок нет.
pub fn forget(id: &str, pool: Address) {
    if let Ok(mut m) = MEMO.lock() {
        m.remove(&(id.to_string(), pool));
    }
}
