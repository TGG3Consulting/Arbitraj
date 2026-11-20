//! Файл: settings/catalog/kraken.rs
//! Что делает: настройки, нужные только бирже Kraken (решение 172), — сколько записей читать в списках Funding (Beta),
//! сколько пар в одном запросе ставок TradeVolume; решение 426 — глубина соединения-страховки стакана. Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Площадки — Kraken»). Решение 379 (Тигран
//! 17.11): venue.deposit_address_claim удалена — claim адреса депозита автоматический (venues/kraken/deposit_addr.rs).

use super::{V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Наибольший limit списков Funding (Beta) — официально «limit (int32, >0, max 500)» у addresses, withdrawals, deposits
/// и deposit/addresses (OpenAPI https://docs.kraken.com/openapi/spot-rest.yaml).
const KRAKEN_LIST_MAX: u64 = 500;

/// Наибольшая глубина канала book — официально «One of: 10, 25, 100, 500, 1000»
/// (https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/book.md; перечень проверяет venues/kraken/ws.rs).
const KRAKEN_BOOK_DEPTH_MAX: u64 = 1000;

/// Настройки решения 172. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static KRAKEN: &[SettingDef] = &[
    d("Площадки", "venue.funding_list_limit", K::Integer, L::Positive, "записей", V, "Решение 172, только Kraken: сколько последних записей читать одной страницей в списках Funding (Beta) — выводы (GET /funding/v1/withdrawals), депозиты (/funding/v1/deposits), адреса вывода (/funding/v1/addresses), выданные адреса депозита (/funding/v2/deposit/addresses). Фильтра по id и хешу у Kraken нет — статус вывода и депозита ищется среди этих записей. Официально не больше 500. Не задана — вывод, статусы и адрес депозита Kraken не работают").max(KRAKEN_LIST_MAX),
    d("Площадки", "venue.book_standby_depth", K::Integer, L::Positive, "уровней", V, "Решение 426, только Kraken: глубина соединения-страховки стакана (канал book WebSocket v2, официально 10, 25, 100, 500 или 1000; не больше venue.depth_levels). Каждое соединение стаканов «market-N» дублируется соединением «standby-N» с теми же парами — у каждого свой локальный стакан со своей проверкой чек-суммы Kraken. Основной потерял стакан пары (чек-сумма, изменение до снимка, отказ подписки, обрыв) — ядру сразу последний сверенный стакан страховки (если он не старше уже отданного), а не «недостоверен» до нового снимка; основной сверен снова — снова основной. Пока работает страховка, ядро видит не больше этой глубины уровней. Цена: вдвое больше соединений (подключение — слот раз в 4 с на IP) и трафика на этой глубине. Журнал venue.book_standby: переключения и потери (интервал log.pool_events_lost_interval_s). Применяется со следующей подпиской. Не задана — страховки нет, как раньше").max(KRAKEN_BOOK_DEPTH_MAX).optional(),
    d("Площадки", "venue.fee_pairs_per_request", K::Integer, L::Positive, "пар", V, "Решение 172, только Kraken: сколько пар в одном запросе ставок комиссии POST /0/private/TradeVolume (pair — список через запятую; предел в документации не указан). Каждый запрос — +1 к счётчику REST ключа (Starter 15, убывание 0,33 в с) — чем больше пар в запросе, тем меньше запросов. Не задана — ставок нет, котировок Kraken нет"),
];
