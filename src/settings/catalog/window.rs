//! Файл: settings/catalog/window.rs
//! Что делает: настройки записи окон (решение 125) — сырые кадры стакана и ленты сделок биржи по монете, пока её
//! направление проходит расчёт прибыльности (и до конца нашей сделки). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Окна»).

use super::{G, V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Наибольший уровень сжатия zstd — ZSTD_maxCLevel() из документации zstd (https://facebook.github.io/zstd/zstd_manual.html).
const ZSTD_MAX_LEVEL: u64 = 22;

/// Наибольший limit снимка стакана MEXC GET /api/v3/depth — официальная документация (Market Data → Order Book: «max 5000»).
const MEXC_DEPTH_MAX: u64 = 5000;

/// Настройки решения 125. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static WINDOW: &[SettingDef] = &[
    d("Окна", "window.enabled", K::Bool, L::None, "да/нет", G, "Решение 125: писать окна — пока направление монеты проходит расчёт прибыльности (и до конца нашей сделки), каждый кадр стакана и каждая сделка биржи по этой монете сохраняются сырыми в файл окна. Нет или не задана любая из window.* — окна не пишутся (причина — на экране «Окна»)"),
    d("Окна", "window.min_hold_ms", K::Integer, L::NonNegative, "мс", G, "Решение 125: окно не закрывается, пока направление не проходит меньше этого (защита от дребезга): прошло снова — то же окно"),
    d("Окна", "window.queue_max", K::Integer, L::Positive, "кадров", G, "Решение 125: сколько кадров одного окна может ждать записи; больше — лишние теряются и считаются («потеряно» в списке окон). Берётся при открытии окна"),
    d("Окна", "window.max_mb", K::Integer, L::Positive, "МБ", G, "Решение 125: лимит места всех файлов окон; больше — удаляются самые старые закрытые, не помогло — запись открытого окна останавливается (причина в списке)"),
    d("Окна", "window.keep_days", K::Integer, L::Positive, "дней", G, "Решение 125: срок хранения закрытых окон (файл и строка списка)"),
    d("Окна", "window.zstd_level", K::Integer, L::Positive, "уровень", G, "Решение 125: уровень сжатия zstd файлов окон, 1–22 (документация zstd: ZSTD_maxCLevel() = 22)").max(ZSTD_MAX_LEVEL),
    d("Окна", "window.cleanup_s", K::Integer, L::Positive, "секунды", G, "Решение 125: как часто уборка окон — закрытые старше window.keep_days, лишнее сверх window.max_mb, файлы без строки в базе. Ещё — при запуске"),
    d("Окна", "venue.trades_interval", K::Choice, L::None, "интервал", V, "Решение 125, только MEXC: интервал канала сделок spot@public.aggre.deals.v3.api.pb@(100ms|10ms)@<symbol> (официальная документация Trade Streams): кадр несёт все сделки за интервал, каждую отдельно с tradeId. Не задан — лента сделок MEXC в окна не пишется (стакан пишется)").choices(&["10ms", "100ms"]).optional(),
    d("Окна", "venue.window_depth_interval", K::Choice, L::None, "интервал", V, "Дополнение решения 125, только MEXC: интервал канала изменений стакана всей глубины spot@public.aggre.depth.v3.api.pb@(100ms|10ms)@<symbol> (официально — Diff.Depth Stream). Не задан — изменения стакана MEXC не пишутся (снимки N уровней пишутся)").choices(&["10ms", "100ms"]).optional(),
    d("Окна", "venue.window_depth_limit", K::Integer, L::Positive, "уровней", V, "Дополнение решения 125, только MEXC: глубина снимка REST GET /api/v3/depth (limit; официально «default 100; max 5000»), с которого начинают изменения стакана, — в начале окна и при разрыве версий (fromVersion ≠ toVersion + 1). Не задана — изменения стакана MEXC не пишутся. Bybit (решение 136): глубина канала orderbook.{depth}.{symbol} окна (снимок и изменения) — 1, 50, 200 или 1000 (официально Orderbook → Depths → Spot); не задана — изменения стакана Bybit не пишутся KuCoin (решение 137): size снимка GET /api/v1/market/orderbook/level2_{size} — только 20 или 100. Binance (решение 160): limit снимка GET /api/v3/depth (1..5000) для изменений стакана окна").max(MEXC_DEPTH_MAX).optional(),
];
