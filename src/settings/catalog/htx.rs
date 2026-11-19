//! Файл: settings/catalog/htx.rs
//! Что делает: настройки, нужные только бирже HTX (решение 171), — поле адреса контракта в справочнике сетей (в
//! документации HTX адреса контракта нет) и размер пачки символов в запросе ставок комиссии; решение 422 — быстрый
//! канал стакана (адрес /feed, уровни, темп запросов снимка). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Площадки — HTX»).

use super::{V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Наибольшие уровни быстрого канала с запросом снимка — официально «REQ supports 5, 20, 150» (venues/htx/fast_ws.rs).
const HTX_FAST_LEVELS_MAX: u64 = 150;
/// Наибольший темп запросов снимка на соединение — официально «100 ms» (venues/htx/fast_ws.rs).
const HTX_FAST_REQ_MAX: u64 = 10;

/// Настройки решений 171 и 422. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static HTX: &[SettingDef] = &[
    d("Площадки", "venue.contract_field", K::Choice, L::None, "поле", V, "Решение 171, только HTX: какое поле ответа GET /v1/settings/common/chains — адрес контракта токена в сети BSC. В официальной документации HTX адреса контракта нет: есть ac «address of chain» (смысл не ясен), в официальном SDK — ещё ca без описания. Задать по живому ответу ?currency=cake (какое поле равно адресу CAKE в BSC). Не задана — адресов контракта нет: монеты HTX не сопоставляются с DEX и другими биржами, сделок с HTX нет (правило «адрес контракта совпадает»)").choices(&["ac", "ca"]).optional(),
    d("Площадки", "venue.fee_symbols_per_req", K::Integer, L::Positive, "символов", V, "Решение 171, только HTX: сколько пар в одном запросе ставок комиссии GET /v2/reference/transact-fee-rate (symbols через запятую; наибольшее число и лимит частоты в документации не указаны — замер на живом). Между запросами — пауза по venue.fee_requests_per_sec. Не задана — ставки HTX не читаются, котировок HTX нет"),
    d("Площадки", "venue.ws_feed_url", K::Url, L::None, "URL", V, "Решение 422, только HTX: адрес быстрого канала стакана (официально wss://api.huobi.pro/feed или wss://api-aws.huobi.pro/feed — «MBP incremental channel and its REQ channel»). Задан — стакан ядру из изменений market.$symbol.mbp.$levels (локальный стакан по seqNum / prevSeqNum, снимок запросом req того же канала), снимки mbp.refresh (venue.ws_url) слушаются параллельно как страховка: разрыв номеров, обрыв, расхождение — сразу снимок и пересинхронизация; переключения — журнал venue.book_fast. Не задан — как раньше: только снимки mbp.refresh").optional(),
    d("Площадки", "venue.fast_book_levels", K::Integer, L::Positive, "уровней", V, "Решение 422, только HTX, при заданном venue.ws_feed_url: уровни быстрого канала market.$symbol.mbp.$levels — 5, 20 или 150 (у 400 нет запроса снимка). 5 и 20 — «tick by tick», но только пары из списка документации (около 30: btcusdt, ethusdt …), остальные пары остаются на снимках; 150 — все пары, изменения раз в 100 мс. Ядру — не больше min(эти уровни, venue.depth_levels). Не задана при заданном адресе — быстрый стакан выключен (журнал), только снимки").max(HTX_FAST_LEVELS_MAX).optional(),
    d("Площадки", "venue.fast_book_req_per_sec", K::Integer, L::Positive, "в секунду", V, "Решение 422, только HTX, при заданном venue.ws_feed_url: сколько запросов снимка {\"req\"} быстрого канала слать в секунду на одно соединение (официально «The limitation of single connection is 100 ms» — не больше 10; превышение — bad-request «429 too many request»). Старт N пар соединения занимает N / это секунд, всё это время пары на снимках. Не задана при заданном адресе — быстрый стакан выключен (журнал), только снимки").max(HTX_FAST_REQ_MAX).optional(),
];
