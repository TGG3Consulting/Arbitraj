//! Файл: settings/scope.rs
//! Что делает: конкретная область значения настройки (вся система, площадка, монета, событие,
//! DEX/вид пула, канал отправки) и её запись в SQLite.
//! Решение 128: id области «DEX» может ввести человек (DEX, которую находит только поиск по фабрике, оракул или
//! события Initialize) — поэтому у id есть правило записи (check_dex_id) и необязательный предел длины нового id
//! (check_new_dex_id_len, настройка dex.id_max_len).
//! Кто вызывает: settings/store.rs, settings/value.rs, settings/read*.rs, api/settings.rs.
//! От чего зависит: settings/schema.rs (ScopeKind), serde.
//! Настройки: dex.id_max_len [вся система] — через store.rs (сюда приходит числом).

use serde::{Deserialize, Serialize};

use super::schema::ScopeKind;

/// Конкретная область значения.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id")]
pub enum Scope {
    Global,
    Venue(String),
    Coin(String),
    Event(String),
    /// DEX («pancakeswap-v3-bsc») — ids даёт адаптер DEX (contract::dex_families: строки с видом, DEX из справочника
    /// пулов и id, где уже есть настройки) или вводит человек (решение 128; правило записи — check_dex_id).
    Dex(String),
    /// Канал отправки («club48») — ids даёт адаптер DEX (contract::send_channels).
    Channel(String),
}

impl Scope {
    /// Вид области. Ошибок нет.
    pub fn kind(&self) -> ScopeKind {
        match self {
            Scope::Global => ScopeKind::Global,
            Scope::Venue(_) => ScopeKind::Venue,
            Scope::Coin(_) => ScopeKind::Coin,
            Scope::Event(_) => ScopeKind::Event,
            Scope::Dex(_) => ScopeKind::Dex,
            Scope::Channel(_) => ScopeKind::Channel,
        }
    }

    /// Строка для хранения в SQLite: «global», «venue:bingx», «coin:CAKE», «event:deal.closed»,
    /// «dex:pancakeswap_v3», «channel:club48».
    pub fn to_key(&self) -> String {
        match self {
            Scope::Global => "global".to_string(),
            Scope::Venue(v) => format!("venue:{v}"),
            Scope::Coin(c) => format!("coin:{c}"),
            Scope::Event(e) => format!("event:{e}"),
            Scope::Dex(d) => format!("dex:{d}"),
            Scope::Channel(c) => format!("channel:{c}"),
        }
    }

    /// Разбор строки из SQLite обратно в область.
    /// Возвращает: Scope или None, если строка не в известном формате.
    pub fn from_key(key: &str) -> Option<Scope> {
        if key == "global" {
            return Some(Scope::Global);
        }
        let (kind, id) = key.split_once(':')?;
        if id.is_empty() {
            return None;
        }
        let id = id.to_string();
        match kind {
            "venue" => Some(Scope::Venue(id)),
            "coin" => Some(Scope::Coin(id)),
            "event" => Some(Scope::Event(id)),
            "dex" => Some(Scope::Dex(id)),
            "channel" => Some(Scope::Channel(id)),
            _ => None,
        }
    }
}

/// Что делает: правило записи id области «DEX» (решение 128): непустой, только строчная латиница, цифры, «-» и «_» —
/// так пишет id DEX индексатор (все 139 id DEX сети bsc в записи GeckoTerminal 2026-10-24 подходят, тест
/// tests/settings_dex_id.rs). Это грамматика ключа, а не порог: числа в нём нет.
/// Принимает: id. Возвращает: Ok(()) или Err(причина по-русски). Ошибки: пустой id, недопустимый символ.
pub fn check_dex_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("Не указан id DEX".into());
    }
    match id
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-' || *c == '_'))
    {
        Some(c) => Err(format!(
            "id DEX «{id}»: недопустимый символ «{c}» — только строчная латиница, цифры, «-» и «_»"
        )),
        None => Ok(()),
    }
}

/// Что делает: предел длины НОВОГО id области «DEX» (решение 128, настройка dex.id_max_len — необязательная).
/// Уже существующие id (есть хоть одно значение) не проверяются: снижение предела не запирает их настройки.
/// Принимает: id, предел (None — не задан: длина не ограничена), есть ли у id уже значения.
/// Возвращает: Ok(()) или Err(причина). Ошибки: новый id длиннее предела.
pub fn check_new_dex_id_len(id: &str, max: Option<u64>, known: bool) -> Result<(), String> {
    match max {
        Some(m) if !known && id.chars().count() as u64 > m => Err(format!(
            "id DEX «{id}» длиннее {m} символов (dex.id_max_len)"
        )),
        _ => Ok(()),
    }
}
