//! Файл: core/quarantine.rs
//! Что делает: карантин монеты по адресу контракта (решение 118, Я-18): долгий перевод «XYZ» (адрес C1) больше не
//! ставит в карантин другой токен «XYZ» (адрес C2). Ключ — CoinKey (адрес, у монеты без адреса — тикер); показ —
//! тикер и короткий адрес. Ручной чёрный список (coin.blocked) остаётся по тикеру, как решил Тигран.
//! В интерфейс (экран «Управление», Risk.quarantine) — списком: id для кнопки «Снять карантин» (адрес или тикер),
//! тикер, подпись, причина.
//! Кто вызывает: core/risk.rs (поле Risk.quarantine, проверка can_open), transfer_watch.rs (поставить и снять),
//! commands.rs (снять вручную).
//! От чего зависит: coin_key.rs, engine.rs.
//! Настройки: нет.

use std::collections::HashMap;

use serde::{Serialize, Serializer};

use crate::core::types::Asset;

use super::coin_key::{self, CoinKey};
use super::engine::{Core, lk};

/// Монета в карантине.
#[derive(Debug, Clone, Serialize)]
pub struct Quarantined {
    /// Что передать в «Снять карантин»: адрес контракта (0x…) или тикер монеты без адреса.
    pub id: String,
    pub ticker: String,
    /// «тикер (0x1234…abcd)».
    pub label: String,
    pub reason: String,
}

/// Карантин: ключ монеты → запись.
pub type Map = HashMap<CoinKey, Quarantined>;

/// Карантин в интерфейс — списком (ключ JSON-объекта может быть только строкой). Ошибки: сериализатора.
pub fn serialize<S: Serializer>(m: &Map, s: S) -> Result<S::Ok, S::Error> {
    let mut list: Vec<&Quarantined> = m.values().collect();
    list.sort_by(|a, b| a.label.cmp(&b.label));
    list.serialize(s)
}

/// id монеты для «Снять карантин»: адрес контракта или тикер. Ошибок нет.
pub fn id_of(a: &Asset) -> String {
    a.bsc_contract
        .map(|c| c.to_string())
        .unwrap_or_else(|| a.ticker.clone())
}

/// Ключ по id из интерфейса: адрес (0x…) — по адресу, иначе тикер. Ошибок нет.
pub fn key_of_id(id: &str) -> CoinKey {
    match id.parse() {
        Ok(a) => CoinKey::Contract(a),
        Err(_) => CoinKey::Ticker(id.to_string()),
    }
}

/// Причина карантина монеты. None — не в карантине. Ошибок нет.
pub fn reason(core: &Core, a: &Asset) -> Option<String> {
    lk(&core.inner.risk)
        .quarantine
        .get(&CoinKey::of(a))
        .map(|q| q.reason.clone())
}

/// Ставит (Some — с причиной) или снимает (None) карантин монеты. Возвращает: снятая запись (если была).
/// Ошибок нет.
pub fn set(core: &Core, a: &Asset, why: Option<String>) -> Option<Quarantined> {
    let key = CoinKey::of(a);
    let old = {
        let mut r = lk(&core.inner.risk);
        match why {
            Some(reason) => {
                let q = Quarantined {
                    id: id_of(a),
                    ticker: a.ticker.clone(),
                    label: coin_key::label(a),
                    reason,
                };
                r.quarantine.insert(key, q)
            }
            None => r.quarantine.remove(&key),
        }
    };
    super::risk::publish(core);
    old
}

/// Снимает карантин по id из интерфейса. Возвращает: снятая запись (None — монеты в карантине не было).
/// Ошибок нет.
pub fn release_id(core: &Core, id: &str) -> Option<Quarantined> {
    let old = lk(&core.inner.risk).quarantine.remove(&key_of_id(id));
    super::risk::publish(core);
    old
}
