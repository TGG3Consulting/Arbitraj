//! Файл: venues/kyberswap/mod.rs
//! Что делает: адаптер DEX через агрегатор KyberSwap (площадка `kyberswap`, решение 91) — реализация контракта
//! gateway/contract.rs. Цены — из REST-API агрегатора (узел для цен не нужен), исполнение — свопом через роутер
//! агрегатора нашим кошельком и узлом BSC. Второй адаптер DEX по тому же контракту; ядро, шлюз и bsc_dex не меняются;
//! выбирается вместо bsc_dex при запуске (kyber.use_instead_of_bsc_dex, main.rs). Всё — строго по официальной
//! документации агрегатора (README.md этой папки); пулов/каналов/графа у агрегатора нет.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
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
