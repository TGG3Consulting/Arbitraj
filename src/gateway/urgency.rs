//! Файл: gateway/urgency.rs
//! Что делает: срочность запросов к площадке (решение 61) — общий для ядра и адаптеров класс задачи. Ядро помечает
//! путь сделки (расчёт кандидата, предпроверка, исполнение) как Trade; адаптер ставит свои классы проверкам и фону.
//! Очередь узла BSC (venues/bsc_dex/node/pace.rs) отдаёт освободившееся место по классу: сначала Trade, потом Check,
//! потом Quote, потом Background — строгий порядок (подтверждено Тиграном, решение 61), с гарантированной долей фона
//! из настройки bsc.node_background_share_pct. Внутри класса — по приходу.
//!   Trade — путь сделки: расчёт кандидата и предпроверка ядра, обмен, вывод, сверки перевода, слежение за транзакцией.
//!   Check — проверки монет и крючков (пробный обмен налогов, пробы крючков, перепроверка).
//!   Quote — котировки живого экрана на каждый тик (ниже проб налогов).
//!   Background — всё остальное: загрузка и проверка пулов, цикл квотера, балансы.
//! Класс задаётся на задачу tokio (task_local): `scope(класс, работа)`; не задан — Background. Задача tokio класс не
//! наследует — его передают при запуске (`scope(current(), …)`).
//! Кто вызывает: core/candidates.rs, core/executor.rs (Trade); адаптер DEX (venues/bsc_dex: mod.rs, pools/market.rs,
//! exec/txs.rs, node/pace.rs).
//! От чего зависит: tokio (task_local).
//! Настройки: нет (доля фона — у очереди узла).

use std::future::Future;

/// Класс срочности. Порядок объявления — порядок очереди (меньше — раньше).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Urgency {
    /// Путь сделки.
    Trade,
    /// Проверки монет и крючков.
    Check,
    /// Котировки живого экрана.
    Quote,
    /// Фон: пулы, квотер-цикл, прочее.
    Background,
}

tokio::task_local! {
    static CLASS: Urgency;
}

/// Класс текущей задачи. Не задан — Background. Ошибок нет.
pub fn current() -> Urgency {
    is_set().unwrap_or(Urgency::Background)
}

/// Класс текущей задачи, если его задали. Ошибок нет.
pub fn is_set() -> Option<Urgency> {
    CLASS.try_with(|c| *c).ok()
}

/// Выполняет работу с классом `class`. Принимает: класс, работу. Возвращает: её результат. Ошибок нет.
pub async fn scope<F: Future>(class: Urgency, work: F) -> F::Output {
    CLASS.scope(class, work).await
}
