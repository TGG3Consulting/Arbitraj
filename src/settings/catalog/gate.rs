//! Файл: settings/catalog/gate.rs
//! Что делает: настройки, нужные только бирже Gate (решение 135), — интервалы каналов WebSocket по её документации,
//! пауза после 429 и глубина поиска ордера по своему ID. Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Площадки — Gate»).

use super::{V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки решения 135. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static GATE: &[SettingDef] = &[
    d("Площадки", "venue.book_interval", K::Choice, L::None, "интервал", V, "Решение 135, только Gate: интервал снимков стакана канала spot.order_book (официально — 100ms или 1000ms, Spot WebSocket v4). Не задан — стаканы Gate не подписываются").choices(&["100ms", "1000ms"]),
    d("Окна", "venue.window_diff_interval", K::Choice, L::None, "интервал", V, "Решение 135, только Gate: интервал канала изменений стакана spot.order_book_update для окон (официально — 20ms при глубине 20 или 100ms при глубине 100). Не задан — изменения стакана Gate в окна не пишутся (снимки пишутся)").choices(&["20ms", "100ms"]).optional(),
    d("Площадки", "venue.rate_limit_pause_ms", K::Integer, L::Positive, "мс", V, "Решение 135, только Gate: после ответа «слишком часто» (HTTP 429, TOO_MANY_REQUESTS) запросы к площадке не отправляются столько миллисекунд, если площадка не прислала Retry-After (в документации Gate его нет; есть заголовок X-Gate-RateLimit-Reset-Timestamp, который транспорт пока не читает). Не задана — паузы нет (в тексте ошибки сказано)"),
    d("Площадки", "venue.order_history_limit", K::Integer, L::Positive, "ордеров", V, "Решение 135, только Gate: сколько законченных ордеров пары читать (GET /spot/orders?status=finished, limit; в документации — «default 100») при поиске ордера по своему ID, если итог постановки неизвестен. Не задана — ордер не ставится (поиск нужен после постановки — проверяется до неё)"),
];
