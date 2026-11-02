//! Файл: venues/kyberswap/mod.rs
//! Что делает: код работы с агрегатором KyberSwap (решение 91). ГЛАВНОЕ применение — ИСПОЛНЕНИЕ свопов: маршрут и
//! транзакцию строит агрегатор (rest.rs /routes + /route/build), отправка — нашим кошельком и узлом BSC (tx.rs,
//! signer.rs, node.rs). Этот код вызывается из bsc_dex при exec.via_kyber = да (bsc_dex остаётся источником сигналов,
//! цен и размера). Сам KyberSwap как ОТДЕЛЬНАЯ площадка-цены (поллинг /routes по всем монетам — market.rs, venue.rs)
//! больше при запуске не регистрируется: агрегатор не рассчитан на поток цен по сотням монет (лимит 3 rps → 403).
//! Всё — строго по официальной документации агрегатора (README.md этой папки).
//! Кто вызывает: bsc_dex (исполнение свопа через state::Parts + trade.rs).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: venue.* [kyberswap], kyber.* (docs/SETTINGS.md); секреты bsc_dex.wallet_key, kyber.node_http_key.

pub mod abi;
pub mod balances;
pub mod coins;
pub mod connection;
pub mod ctx;
pub mod errors;
pub mod market;
pub mod node;
pub mod num;
pub mod quote;
pub mod rest;
pub mod signer;
pub mod state;
pub mod trade;
pub mod transfer;
pub mod tx;
pub mod venue;

use state::Parts;

/// Адаптер DEX через агрегатор KyberSwap.
pub struct Kyberswap {
    p: Parts,
}

impl Kyberswap {
    /// Создаёт адаптер. Ничего не подключает: соединения открываются по запросу ядра. Ошибок нет.
    pub fn new(ctx: ctx::Ctx) -> Self {
        Self { p: Parts::new(ctx) }
    }
}
