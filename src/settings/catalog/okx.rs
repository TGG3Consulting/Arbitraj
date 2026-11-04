//! Файл: settings/catalog/okx.rs
//! Что делает: настройки, нужные только бирже OKX (решение 161), — адрес канала business (депозиты событиями), темп
//! подключений WebSocket по её документации, опрос итога перевода между счетами Funding ↔ Trading. Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Площадки — OKX»).

use super::{V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Подключений в секунду — официально «3 requests per second (based on IP)» (как venues/okx/market.rs).
const OKX_CONNECTS_MAX: u64 = 3;

/// Настройки решения 161. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static OKX: &[SettingDef] = &[
    d("Площадки", "venue.ws_business_url", K::Url, L::None, "URL", V, "Решение 161, только OKX: адрес WebSocket business (канал deposit-info — зачисления депозитов событием с хешем, нужен вход). Основной домен — wss://ws.okx.com/ws/v5/business (официально «Overview → Production Trading Services»); у счёта другой сущности (EEA, US) — свой домен. Нужен при venue.balance_events = да; не задан — поток аккаунта OKX не запускается (зачисление — опросом статуса депозита)").optional(),
    d("Площадки", "venue.ws_connects_per_sec", K::Integer, L::Positive, "в секунду", V, "Решение 161, только OKX: сколько WebSocket-соединений стаканов открывать в секунду (официально «Connection limit: 3 requests per second (based on IP)» — не больше 3). Не задана — стаканы OKX не подписываются").max(OKX_CONNECTS_MAX),
    d("Площадки", "venue.transfer_poll_ms", K::Integer, L::Positive, "мс", V, "Решение 161, только OKX: как часто спрашивать итог перевода между своими счетами Funding ↔ Trading (GET /api/v5/asset/transfer-state; ответ на перевод «does not necessarily reflect the actual transfer result»). Лимит эндпоинта перевода — 2 запроса/с на монету. Не задана — перевод не отправляется"),
    d("Площадки", "venue.transfer_wait_ms", K::Integer, L::Positive, "мс", V, "Решение 161, только OKX: сколько ждать итог перевода Funding ↔ Trading (success / failed). Не дождались — «итог неизвестен», без повторов (ручная проверка). Не задана — перевод не отправляется"),
];
