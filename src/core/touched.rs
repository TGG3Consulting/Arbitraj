//! Файл: core/touched.rs
//! Что делает: тип Touched — что изменилось в рыночном состоянии после события адаптера.
//! Кто вызывает: core/market_state.rs (создаёт), core/tick.rs (по нему решает, что пересчитать).
//! От чего зависит: core/types.
//! Настроек нет.

use alloy_primitives::Address;

use crate::core::types::{
    CoinCheck, PoolCheck, SendChannelStats, VenueId, WalletQueue, WalletQueueState,
};

/// Что изменилось после события — по этому ядро решает, что пересчитать.
#[derive(Debug, Clone)]
pub enum Touched {
    /// Новый стакан монеты. `first` — стакан этой монеты на площадке пришёл впервые (или после сброса).
    Book {
        venue: VenueId,
        ticker: String,
        first: bool,
    },
    /// Новое состояние пула. `first` — пул встречен впервые.
    Pool {
        venue: VenueId,
        tokens: (Address, Address),
        first: bool,
    },
    /// Площадка объявила данные монеты недостоверными — стакан удалён.
    Invalidated {
        venue: VenueId,
        ticker: String,
    },
    CoinCheck(CoinCheck),
    PoolCheck(PoolCheck),
    Channel(SendChannelStats),
    /// Очередь кошелька DEX изменилась: новое и прошлое состояние (уведомления — core/tick.rs).
    WalletQueue {
        queue: WalletQueue,
        prev: Option<WalletQueueState>,
    },
    /// Биржа сообщила монеты с торгуемой спот-парой — пересборка справочника и подписок.
    SpotPairs,
    /// DEX сообщил монеты с ещё не прочитанными пулами; `changed` — список изменился (пересборка справочника).
    PoolsLoading {
        changed: bool,
    },
    /// Адаптер DEX закончил первый проход проверки налогов: [прошли, не прошли, ждут пула, не задана настройка, пулы
    /// ещё загружаются].
    CheckPass {
        venue: VenueId,
        counts: [usize; 5],
    },
    /// Сообщение адаптера для уведомлений.
    Notice {
        venue: VenueId,
        code: String,
        text: String,
    },
}
