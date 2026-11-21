//! Файл: venues/bsc_dex/pools/events_crypto.rs
//! Что делает: события crypto-пула Curve (решение 442: Twocrypto-NG, Tricrypto-NG) — topic0 для подписки и применение:
//! любое известное событие пула (обмен, ликвидность, параметры комиссии, рампа A/gamma, доля админа) → перечитать пул
//! целиком. Суммы событий не разбираются: перечитывание на блоке события точно (обмен меняет и price_scale — tweak_price).
//! Состояние пула меняется только его вызовами (каждый пишет событие) — каждым блоком вид не перечитывается; кроме
//! рампы A/gamma: A и gamma меняются по времени блока без событий — такой пул (CryptoData.moving) перечитывается каждым
//! блоком, пока рампа идёт (решение 453, per_block.rs). Решение 453: события пулов crypto v1 (tricrypto 2021, фабрика
//! crypto v1) — свои подписи (abi_crypto_v1.rs), применение то же.
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_crypto.rs, abi_crypto_v1.rs (topic0 и ссылки на официальный исходник), events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::events::Applied;
use super::registry::Entry;
use super::{abi_crypto, abi_crypto_v1};

/// topic0 всех событий пула (NG и v1 — решение 453; подписка общая на все crypto-виды). Ошибок нет.
pub fn topics() -> Vec<B256> {
    let mut out = abi_crypto::topics().to_vec();
    out.extend(abi_crypto_v1::topics().iter().copied());
    out
}

/// topic0 событий изменения ликвидности (мелкие пулы слушают только их, pools/depth.rs), NG и v1. Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    let mut out = abi_crypto::liquidity_topics();
    out.extend(abi_crypto_v1::liquidity_topics());
    out
}

/// Применяет событие: известное событие пула → Refresh, иначе Ignored. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if abi_crypto::topics().contains(t) || abi_crypto_v1::topics().contains(t) => {
            Applied::Refresh(format!(
                "{}: событие crypto-пула Curve — перечитать состояние",
                e.family.title
            ))
        }
        _ => Applied::Ignored,
    }
}
