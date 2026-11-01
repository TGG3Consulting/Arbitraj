//! Файл: venues/kyberswap/state.rs
//! Что делает: общее состояние адаптера kyberswap в памяти — окружение, список активных монет подписки, кэш decimals
//! токенов, строгая очередь кошелька (одна транзакция за раз, решение 45), задача и состояние поллинга квот.
//! Кто вызывает: mod.rs (создаёт), venue.rs, market.rs, trade.rs, transfer.rs, balances.rs, coins.rs, quote.rs.
//! От чего зависит: ctx.rs, core/types (Asset), tokio, alloy-primitives.
//! Настройки: нет.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex, RwLock};

use alloy_primitives::Address;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::core::types::Asset;

use super::ctx::Ctx;

/// Состояние адаптера. Дешёво клонируется.
#[derive(Clone)]
pub struct Parts {
    pub ctx: Ctx,
    /// Монеты последней подписки ядра (core/subscribe.rs передаёт dex_tradable + USDT + промежуточные).
    assets: Arc<RwLock<Vec<Asset>>>,
    /// Кэш decimals токенов (читается с контракта один раз).
    decimals: Arc<RwLock<HashMap<Address, u8>>>,
    /// Строгая очередь кошелька: в работе одна транзакция за раз (решение 45) — approve и своп одной сделки,
    /// перевод. nonce читается у узла на «latest», предыдущая транзакция к этому моменту уже в блоке (ждём квитанцию).
    pub wallet_lock: Arc<Mutex<()>>,
    /// Задача поллинга квот и множество монет с текущим маршрутом (для события Invalidated при пропаже маршрута).
    poll: Arc<StdMutex<Poll>>,
}

/// Поллинг квот: активная задача и монеты, по которым сейчас есть маршрут.
#[derive(Default)]
pub struct Poll {
    pub task: Option<JoinHandle<()>>,
    pub routed: HashSet<Address>,
}

impl Parts {
    /// Пустое состояние. Ошибок нет.
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            assets: Arc::default(),
            decimals: Arc::default(),
            wallet_lock: Arc::default(),
            poll: Arc::new(StdMutex::new(Poll::default())),
        }
    }

    /// Заменяет список активных монет. Ошибок нет.
    pub fn set_assets(&self, assets: Vec<Asset>) {
        *self.assets.write().unwrap_or_else(|p| p.into_inner()) = assets;
    }

    /// Текущий список активных монет. Ошибок нет.
    pub fn assets(&self) -> Vec<Asset> {
        self.assets
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Запоминает decimals токена. Ошибок нет.
    pub fn remember_decimals(&self, token: Address, d: u8) {
        self.decimals
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(token, d);
    }

    /// decimals токена из кэша. None — ещё не известны. Ошибок нет.
    pub fn decimals_known(&self, token: Address) -> Option<u8> {
        self.decimals
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&token)
            .copied()
    }

    /// Останавливает прежнюю задачу поллинга и ставит новую. Возвращает: монеты, у которых был маршрут (для их
    /// инвалидации новой задачей). Ошибок нет.
    pub fn swap_poll(&self, task: JoinHandle<()>) {
        let mut p = self.poll.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(old) = p.task.take() {
            old.abort();
        }
        p.task = Some(task);
    }

    /// Обновляет множество монет с маршрутом; возвращает (пропавшие — нужна инвалидация, появившиеся — новые).
    /// Ошибок нет.
    pub fn update_routed(&self, now: HashSet<Address>) -> (Vec<Address>, Vec<Address>) {
        let mut p = self.poll.lock().unwrap_or_else(|p| p.into_inner());
        let gone: Vec<Address> = p.routed.difference(&now).copied().collect();
        let entered: Vec<Address> = now.difference(&p.routed).copied().collect();
        p.routed = now;
        (gone, entered)
    }
}
