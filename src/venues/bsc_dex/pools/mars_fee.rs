//! Индикативная фиксированная цена одношагового MarSwap из того же чтения, что его резервы.
//! Перед отправкой кошелёк отдельно перечитывает lpInfo() и подписывает точное количество wei.
use alloy_primitives::{Address, U256};
use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
type Quotes = VecDeque<((u64, Address), u64, U256)>;
static RECENT: LazyLock<Mutex<Quotes>> = LazyLock::new(Mutex::default);
pub(super) fn record(chain: u64, factory: Address, block: u64, wei: U256) {
    let key = (chain, factory);
    let mut recent = RECENT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(row) = recent.iter_mut().find(|row| row.0 == key) {
        if block >= row.1 {
            *row = (key, block, wei)
        }
        return;
    }
    if recent.len() == 64 {
        recent.pop_front();
    }
    recent.push_back((key, block, wei));
}
pub(in crate::venues::bsc_dex) fn indicative(
    chain: u64,
    factory: Address,
    block: u64,
) -> Option<U256> {
    RECENT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|row| row.0 == (chain, factory) && row.1 <= block)
        .map(|row| row.2)
}
