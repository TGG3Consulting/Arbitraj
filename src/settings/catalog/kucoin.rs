//! Файл: settings/catalog/kucoin.rs
//! Что делает: настройки, нужные только бирже KuCoin, — решение 423: темп снимков стакана REST Get Full OrderBook для
//! быстрого стакана (изменения /market/level2 + снимок; страховка — снимки /spotMarket/level2Depth5|50). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Площадки и соединения»).

use super::{V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки решения 423. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static KUCOIN: &[SettingDef] = &[
    d("Площадки", "venue.full_book_per_sec", K::Integer, L::Positive, "в секунду", V, "Решение 423, только KuCoin: быстрый стакан — изменения /market/level2 в реальном времени (локальный стакан по процедуре KuCoin со снимком REST GET /api/v3/market/orderbook/level2 — Get Full OrderBook, подписанный: нужны ключи и venue.api_key_version), снимки /spotMarket/level2Depth5|50 — страховка параллельно: разрыв номеров или расхождение — сразу снимки, затем пересинхронизация не раньше venue.ws_reconnect_pause_ms. Значение — сколько снимков Get Full OrderBook в секунду, одна очередь на все соединения: вес 3 из пула Spot (по UID, общий с ордерами; VIP0 — 4000 за 30 с), N в секунду — 90 × N веса за 30 с; старт M пар — M / N секунд. Включение и выключение — с нового подключения соединения. Не задана — быстрого стакана нет, стакан только из снимков (как до решения 423)").optional(),
];
