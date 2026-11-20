//! Файл: venues/bsc_dex/pools/events_crypto.rs
//! Что делает: события crypto-пула Curve (решение 442: Twocrypto-NG, Tricrypto-NG) — topic0 для подписки и применение:
//! любое известное событие пула (обмен, ликвидность, параметры комиссии, рампа A/gamma, доля админа) → перечитать пул
//! целиком. Суммы событий не разбираются: перечитывание на блоке события точно (обмен меняет и price_scale — tweak_price).
//! Состояние пула меняется только его вызовами (каждый пишет событие) — каждым блоком вид не перечитывается; рампа
//! A/gamma, идущая по времени без событий, держит пул остановленным до следующего события (load_crypto.rs).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_crypto.rs (topic0 и ссылки на официальный исходник), events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::abi_crypto;
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пула. Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_crypto::topics().to_vec()
}

/// topic0 событий изменения ликвидности (мелкие пулы слушают только их, pools/depth.rs). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    abi_crypto::liquidity_topics()
}

/// Применяет событие: известное событие пула → Refresh, иначе Ignored. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if abi_crypto::topics().contains(t) => Applied::Refresh(format!(
            "{}: событие crypto-пула Curve — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}
