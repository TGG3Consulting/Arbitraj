//! Файл: settings/scope.rs
//! Что делает: конкретная область значения настройки (вся система, площадка, монета, событие,
//! DEX/вид пула, канал отправки) и её запись в SQLite.
//! Кто вызывает: settings/store.rs, settings/value.rs, settings/read*.rs, api/settings.rs.
//! От чего зависит: settings/schema.rs (ScopeKind), serde.
//! Настройки: нет.

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
    /// DEX («pancakeswap») или вид пула («pancakeswap_v3») — ids даёт адаптер DEX (contract::dex_families).
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
