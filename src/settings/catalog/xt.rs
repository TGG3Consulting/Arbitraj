//! Файл: settings/catalog/xt.rs
//! Что делает: настройки, нужные только бирже XT.com (решение 167), — канал стакана (снимки или изменения), очередь
//! снимков REST для локального стакана, адреса фьючерсов Coin-M и user-center. Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Площадки — XT»).

use super::{V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Снимков стакана в секунду — официально «10/s/ip» (GET /v4/public/depth, как venues/xt/snapshots.rs).
const XT_SNAPSHOTS_MAX: u64 = 10;
/// Наибольший limit снимка — официально «1~500» (как venues/xt/snapshots.rs).
const XT_SNAPSHOT_LIMIT_MAX: u64 = 500;

/// Настройки решения 167. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static XT: &[SettingDef] = &[
    d("Площадки", "venue.book_stream", K::Choice, L::None, "канал", V, "Решение 167, только XT: какой канал стакана слушать. depth — снимки depth@{symbol},{levels} (уровни 5, 10, 20, 50) раз в 1000 мс: просто, но медленно; depth_update — изменения depth_update@{symbol} раз в 100 мс + снимок REST в начале и после разрыва номеров (локальный стакан по инструкции XT «Orderbook manage»; ядру — venue.depth_levels верхних уровней). Не задана — стаканы XT не подписываются").choices(&["depth", "depth_update"]),
    d("Площадки", "venue.depth_snapshot_per_sec", K::Integer, L::Positive, "в секунду", V, "Решение 167, только XT, при venue.book_stream = depth_update: сколько снимков стакана REST (GET /v4/public/depth) запрашивать в секунду — одна очередь на все соединения (официально «10/s/ip»; 429 — блокировка IP). Старт N пар занимает N / это секунд. Не задана — локальный стакан не собирается").max(XT_SNAPSHOTS_MAX),
    d("Площадки", "venue.depth_snapshot_limit", K::Integer, L::Positive, "уровней", V, "Решение 167, только XT, при venue.book_stream = depth_update: глубина снимка REST (limit, официально 1–500; инструкция «Orderbook manage» советует 500). Уровни вне снимка без изменений в локальном стакане не видны. Не задана — локальный стакан не собирается").max(XT_SNAPSHOT_LIMIT_MAX),
    d("Площадки", "venue.futures_coin_rest_url", K::Url, L::None, "URL", V, "Решение 167, только XT, панель «Балансы» (только чтение): адрес REST фьючерсов Coin-M (официально https://dapi.xt.com); USDT-M — venue.futures_rest_url (https://fapi.xt.com). Не задан — счёт «Фьючерсы Coin-M» показывает причину").optional(),
    d("Площадки", "venue.user_center_url", K::Url, L::None, "URL", V, "Решение 167, только XT: адрес user-center (официально https://api.xt.com) — «Проверить подключение» читает свою запись ключа GET /v4/user/account/api-key: блокировка, право торговли, привязка к IP (секрет ключа из ответа не пишется в журнал). Не задан — проверяется только, что ключ и подпись приняты").optional(),
];
