//! Файл: settings/catalog/bitget.rs
//! Что делает: настройки, нужные только бирже Bitget (решение 162), — окно поиска в истории депозитов и выводов
//! (у Bitget startTime и endTime обязательны). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Площадки и соединения»).

use super::{V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки решения 162. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static BITGET: &[SettingDef] = &[
    d("Площадки", "venue.history_window_s", K::Integer, L::Positive, "с", V, "Решение 162, только Bitget: на сколько секунд назад от «сейчас» искать запись в истории депозитов и выводов (GET /api/v2/spot/wallet/deposit-records и withdrawal-records: startTime и endTime обязательны, наибольший промежуток в документации не указан; страница — до 100 записей). Нужно для статуса депозита по хешу, статуса вывода и поиска вывода по своему clientOid. Не задана — вывод с Bitget не отправляется (искать его потом нечем), статусы не читаются"),
];
