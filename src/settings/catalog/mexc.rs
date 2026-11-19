//! Файл: settings/catalog/mexc.rs
//! Что делает: настройки, нужные только бирже MEXC, — решение 420: быстрый стакан (канал изменений
//! spot@public.aggre.depth.v3.api.pb + снимок REST по официальной процедуре) при страховке потоком снимков. Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Площадки и соединения», MEXC).

use super::{V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Снимков стакана в секунду: «IP-weighted endpoints share 300 weight per 10 seconds» / «Weight(IP): 3» (GET
/// /api/v3/depth) = 10 (как venues/mexc/fast_snap.rs MAX_PER_SEC).
const MEXC_SNAPSHOTS_MAX: u64 = 10;
/// Наибольший limit снимка — Order Book «max 5000» (как venues/mexc/fast_snap.rs MAX_LIMIT).
const MEXC_SNAPSHOT_LIMIT_MAX: u64 = 5000;

/// Настройки решения 420. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static MEXC: &[SettingDef] = &[
    d("Площадки", "venue.fast_book_interval", K::Choice, L::None, "интервал", V, "Решение 420, только MEXC: быстрый стакан — интервал канала изменений spot@public.aggre.depth.v3.api.pb@(10ms|100ms)@<SYMBOL> (Diff.Depth Stream). Стакан пары собирается в памяти по официальной процедуре (снимок REST GET /api/v3/depth + fromVersion = прежний toVersion + 1) и отдаётся ядру (venue.depth_levels верхних уровней), пока синхронизирован; поток снимков spot@public.limit.depth.v3.api.pb (раз в ~500 мс) слушается параллельно как страховка: разрыв версий, сбой, отказ подписки — ядру сразу последний кадр снимков, быстрый стакан пересинхронизируется. На пару 2 подписки — монет на соединение не больше 15. Переключения и пересинхронизации — строка журнала venue.fast_book. Применяется с новой сессией соединения. Не задана — только снимки, как раньше").choices(&["10ms", "100ms"]).optional(),
    d("Площадки", "venue.fast_book_snapshot_per_sec", K::Integer, L::Positive, "в секунду", V, "Решение 420, только MEXC, при venue.fast_book_interval: сколько снимков REST GET /api/v3/depth (вес 3 из корзины IP 300 / 10 с; после 429 — бан IP) для быстрых стаканов запрашивать в секунду — одна очередь на все соединения; старт N пар занимает N / это секунд (всё это время пара торгуется по снимкам). Учитывается и ограничителем venue.public_ip_weight_10s. Не задана — быстрого стакана нет (журнал), только снимки").max(MEXC_SNAPSHOTS_MAX).optional(),
    d("Площадки", "venue.fast_book_snapshot_limit", K::Integer, L::Positive, "уровней", V, "Решение 420, только MEXC, при venue.fast_book_interval: глубина снимка REST (limit, официально «max 5000»; процедура MEXC берёт 5000), с которого собирается быстрый стакан. Уровни вне снимка без изменений в быстром стакане не видны (примечание процедуры MEXC). Не задана — быстрого стакана нет (журнал), только снимки").max(MEXC_SNAPSHOT_LIMIT_MAX).optional(),
];
