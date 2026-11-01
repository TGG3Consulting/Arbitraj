//! Файл: venues/bsc_dex/mod.rs
//! Что делает: адаптер DEX в сети BSC (PancakeSwap, Uniswap; один кошелёк, один узел) — реализация контракта
//! gateway/contract.rs. Всё общение с узлом и контрактами — внутри этой папки; наружу — типы ядра и единые ошибки.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: docs/SETTINGS.md — «Узел BSC», «DEX», «Отправка транзакций», venue.* и feed.* с областью bsc_dex.
//!
//! Только по официальной документации и данным самого блокчейна (CLAUDE.md, решение 23). Источники —
//! README.md этой папки и шапки файлов. Разработка по шагам Issue #7: 1a (узел, кошелёк, проверка подключения),
//! 1b (пулы), 1c (математика, котировка, проверки), 1d (исполнение, каналы отправки), 1e (умный режим).
//! Здесь — сам адаптер (BscDex); общее состояние — parts.rs; функции контракта — venue.rs.

pub mod check;
pub mod coins;
pub mod connection;
pub mod connection_extra;
pub mod ctx;
pub mod ctx_url;
pub mod directory;
pub mod errors;
pub mod exec;
pub mod math;
pub mod node;
pub mod num;
pub mod parts;
pub mod pools;
pub mod quote;
pub mod resubscribe;
pub mod send;
pub mod settings_wait;
pub mod tokens;
pub mod venue;
pub mod wallet;

use parts::Parts;

/// Адаптер DEX.
pub struct BscDex {
    p: Parts,
    /// Подписка на рынок: задачи, последний список, пересборка при изменении настроек промежуточных монет.
    subs: resubscribe::Subs,
    /// Исполнение свопа через агрегатор KyberSwap (решение 91, настройка exec.via_kyber): свой контекст из тех же
    /// зависимостей. Сигналы, цены и подбор размера остаются на bsc_dex; KyberSwap — только маршрут и своп на сделке.
    kyber: crate::venues::kyberswap::state::Parts,
}

impl BscDex {
    /// Создаёт адаптер. Ничего не подключает: соединения открываются по запросу ядра.
    pub fn new(ctx: ctx::Ctx) -> Self {
        // Контекст KyberSwap-исполнения из общих зависимостей (настройки, секреты, HTTP, журнал, сырые логи).
        let kyber = crate::venues::kyberswap::state::Parts::new(crate::venues::kyberswap::ctx::Ctx {
            settings: ctx.settings.clone(),
            vault: ctx.vault.clone(),
            http: ctx.http.clone(),
            journal: ctx.journal.clone(),
            raw: ctx.raw.clone(),
        });
        Self {
            p: Parts::new(ctx),
            subs: resubscribe::Subs::default(),
            kyber,
        }
    }

    /// Пересобирать подписку при изменении dex.intermediate_* (решение 52) — resubscribe.rs. Принимает: приёмник
    /// изменений настроек. Возвращает: задачу наблюдателя. Ошибок нет.
    pub fn watch_settings(
        &self,
        rx: tokio::sync::broadcast::Receiver<crate::settings::live::Change>,
    ) -> tokio::task::JoinHandle<()> {
        self.subs.watch(self.p.clone(), rx)
    }
}
