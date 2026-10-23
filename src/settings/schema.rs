//! Файл: settings/schema.rs
//! Что делает: описание настройки — имя, тип значения, ограничение, единица, область, описание.
//! Значений по умолчанию нет и быть не может (CLAUDE.md, docs/SETTINGS.md).
//! Кто вызывает: settings/catalog.rs (список настроек), settings/store.rs (проверка), api (схема для интерфейса).
//! От чего зависит: serde.
//! Настройки: нет.

use serde::Serialize;

/// Тип значения настройки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Kind {
    /// Десятичное число (деньги, проценты, коэффициенты). Хранится строкой, без потери точности.
    Decimal,
    /// Целое неотрицательное число (мс, секунды, штуки).
    Integer,
    /// Да / нет.
    Bool,
    /// Произвольный текст (например, ID чата Telegram).
    Text,
    /// Адрес только с защищённым соединением: https или wss.
    Url,
    /// Список строк (например, список площадок).
    TextList,
    /// IP-адрес (IPv4 или IPv6).
    Ip,
}

/// Ограничение значения, вытекающее из смысла настройки (не порог и не число из головы).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Limit {
    /// Без ограничения сверх типа.
    None,
    /// Больше нуля (время, количество, сумма лимита).
    Positive,
    /// Ноль или больше.
    NonNegative,
    /// Доля от 0 до 1 включительно.
    Fraction,
    /// Процент от 0 до 100 включительно.
    Percent,
}

/// Где действует настройка. Значение для более узкой области сильнее общего:
/// монета сильнее площадки, площадка сильнее всей системы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ScopeKind {
    /// Вся система.
    Global,
    /// Отдельная площадка (биржа или DEX).
    Venue,
    /// Отдельная монета.
    Coin,
    /// Отдельное событие уведомлений.
    Event,
}

/// Описание одной настройки.
#[derive(Debug, Clone, Serialize)]
pub struct SettingDef {
    /// Имя, как в docs/SETTINGS.md (например «money.min_profit_usd»).
    pub name: &'static str,
    pub kind: Kind,
    pub limit: Limit,
    /// Единица измерения для интерфейса.
    pub unit: &'static str,
    /// Допустимые области; первая — основная.
    pub scopes: &'static [ScopeKind],
    /// Что означает — по-русски, для интерфейса.
    pub description: &'static str,
    /// Группа экрана «Настройки».
    pub group: &'static str,
}
