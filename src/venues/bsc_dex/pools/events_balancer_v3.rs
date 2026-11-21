//! Файл: venues/bsc_dex/pools/events_balancer_v3.rs
//! Что делает: решение 441 (T433-F2) — события пула Balancer V3: topic0 для подписки и применение — известное событие
//! самого пула (AmpUpdateStarted, AmpUpdateStopped стейблсвапа) → перечитать пул целиком. Балансы, комиссию и паузу
//! пула меняет хранилище Vault V3 событиями от СВОЕГО адреса (Swap, LiquidityAdded, LiquidityRemoved,
//! SwapFeePercentageChanged, PoolPausedStateChanged — IVaultEvents.sol), курсы токенов меняют поставщики курса без событий
//! пула — подписка по адресу пула их не видит, поэтому вид ещё перечитывается каждым блоком (pools/per_block.rs).
//! Решение 452: событие хранилища PoolRegistered (IVaultEvents.sol L26-L35: пул и фабрика — indexed, токены — в
//! tokenConfig) — новый пул без индексатора: кандидат для проверки (pools/created_log.rs; проверка — та же
//! verify_balancer_v3.rs: фабрика из списков, крючок, токены). Пул после регистрации ещё не инициализирован — до
//! PoolInitialized он на паузе (load_balancer_v3.rs: !isPoolInitialized), затем перечитывается каждым блоком.
//! Кто вызывает: pools/events.rs, pools/created_log.rs (registered, topic).
//! От чего зависит: abi_balancer_v3.rs, abi_balancer_v3_kinds.rs, events.rs, registry.rs, verify.rs (Candidate).
//! Настройки: нет.

use std::collections::HashMap;

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256};

use super::abi_balancer_v3;
use super::abi_balancer_v3_kinds::IBalancerV3VaultEvents::PoolRegistered;
use super::events::Applied;
use super::families::Family;
use super::registry::Entry;
use super::verify::Candidate;

/// topic0 событий пула (abi_balancer_v3.rs; StablePool.sol L77, L83). Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_balancer_v3::topics()
}

/// topic0 событий изменения ликвидности самого пула — нет (ликвидность меняет хранилище: LiquidityAdded /
/// LiquidityRemoved от адреса Vault). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    Vec::new()
}

/// Применяет событие: известное событие пула → Refresh, иначе Ignored. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics().contains(t) => Applied::Refresh(format!(
            "{}: событие пула Balancer V3 — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}

/// topic0 события PoolRegistered хранилища (источник — dex.vault, pools/created_log.rs). Ошибок нет.
pub fn registered_topic() -> B256 {
    PoolRegistered::SIGNATURE_HASH
}

/// Кандидат из PoolRegistered: монета — первый токен пула, который слушается (coins) и не встречный; встречный токен
/// должен быть среди токенов пула. Принимает: вид DEX, лог, встречный токен, слушаемые монеты. Возвращает: кандидата
/// или None (в пуле нет встречного токена или слушаемой монеты). Ошибки (текст): лог не по ABI.
pub fn registered(
    f: &'static Family,
    log: &Log,
    quote: Address,
    coins: &HashMap<Address, String>,
) -> Result<Option<Candidate>, String> {
    let e = PoolRegistered::decode_log_data(&log.inner.data)
        .map_err(|e| format!("PoolRegistered (Balancer V3) не по ABI: {e}"))?;
    let tokens: Vec<Address> = e.tokenConfig.iter().map(|t| t.token).collect();
    if !tokens.contains(&quote) {
        return Ok(None);
    }
    let coin = tokens
        .iter()
        .find(|t| **t != quote && coins.contains_key(*t));
    Ok(coin.map(|c| Candidate {
        family: f,
        pool: format!("{:#x}", e.pool),
        coin: *c,
        quote,
    }))
}
