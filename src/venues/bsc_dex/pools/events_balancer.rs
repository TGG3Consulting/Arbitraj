//! Файл: venues/bsc_dex/pools/events_balancer.rs
//! Что делает: события пула Balancer V2 (решение 289) — topic0 для подписки и применение: любое известное событие самого
//! пула (SwapFeePercentageChanged, PausedStateChanged, AmpUpdateStarted, AmpUpdateStopped) → перечитать пул целиком.
//! Балансы пула меняет хранилище (Vault) своими событиями от СВОЕГО адреса — подписка по адресу пула их не видит, поэтому
//! вид ещё перечитывается каждым блоком (pools/per_block.rs). Событий ликвидности у самого пула нет (вход и выход
//! ликвидности — PoolBalanceChanged хранилища).
//! Решение 473: у ключа с dex.balancer_pool (вид запомнен при чтении пула — balancer_pool_type.rs seen) — ещё события
//! своего вида (кеш курсов, поставщики, веса по времени, swapEnabled, комиссия по времени, пределы цены BPT, токены
//! managed, цели linear — abi_balancer_more.rs topics) → перечитать пул. Ключ без вида (BSC) — прежние четыре.
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_balancer.rs (topic0 и ссылки на исходник), events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::abi_balancer;
use super::events::Applied;
use super::families::Family;
use super::registry::Entry;

/// topic0 событий пула (abi_balancer.rs; BasePool.sol L88, ITemporarilyPausable.sol L24, StablePool.sol L47-L48;
/// решение 473 — события вида ключа, abi_balancer_more.rs). Принимает: вид DEX. Ошибок нет.
pub fn topics(f: &Family) -> Vec<B256> {
    let mut v = abi_balancer::topics();
    if let Some(t) = super::balancer_pool_type::seen(f) {
        v.extend(super::abi_balancer_more::topics(t));
    }
    v
}

/// topic0 событий изменения ликвидности самого пула — нет (ликвидность меняет хранилище, IVault.sol L441 PoolBalanceChanged,
/// лог от адреса хранилища). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    Vec::new()
}

/// Применяет событие: известное событие пула → Refresh, иначе Ignored (TokenAdded / TokenRemoved — и сброс
/// неизменяемого пула). Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    // Решение 473: managed добавил / убрал токен — неизменяемое пула (токены, decimals, BPT) сбросить до перечитывания.
    let tokens = super::abi_balancer_more::token_set_changed();
    if log.topic0().is_some_and(|t| tokens.contains(t)) {
        super::load_balancer_fixed::forget(log.address());
    }
    match log.topic0() {
        Some(t) if topics(e.family).contains(t) => Applied::Refresh(format!(
            "{}: событие пула Balancer V2 — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}
