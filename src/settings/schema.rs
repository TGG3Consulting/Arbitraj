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
    /// Один вариант из списка `SettingDef::choices` (например режим обмена «direct» / «smart»).
    Choice,
    /// Адрес в сети BSC (EVM): 0x и 40 шестнадцатеричных знаков.
    Address,
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
    /// DEX или вид пула у адаптера DEX: id — «pancakeswap» (вся DEX) или «pancakeswap_v3» (вид пула).
    /// Значение задаётся ровно для своего id, без подстановки из других областей.
    Dex,
    /// Канал отправки транзакций (id — из списка каналов адаптера DEX, например «club48»).
    /// Значение задаётся ровно для своего канала, без подстановки из других областей.
    Channel,
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
    /// Для Kind::Choice — допустимые варианты (значения из решений Тиграна); для остальных — пусто.
    pub choices: &'static [&'static str],
    /// Верхняя граница целого числа, если она задана решением или протоколом (ссылка — в описании);
    /// None — без верхней границы.
    pub max: Option<u64>,
}

impl SettingDef {
    /// Задаёт варианты выбора (для Kind::Choice). Принимает: список. Возвращает: описание с вариантами.
    pub const fn choices(self, choices: &'static [&'static str]) -> Self {
        SettingDef { choices, ..self }
    }

    /// Задаёт верхнюю границу целого. Принимает: границу. Возвращает: описание с границей.
    pub const fn max(self, max: u64) -> Self {
        SettingDef {
            max: Some(max),
            ..self
        }
    }
}
