//! Файл: venues/bsc_dex/pools/graph_timing.rs
//! Что делает: замер построения графа маршрутов (решение 301, часть д; задача t132) — одна запись журнала
//! `dex.graph_build` на построение: сколько построение ждало замок (одно построение за раз, pools/graph_cache.rs),
//! сколько ещё ждали вместе с ним при входе и за ним при получении замка, время build, N — пулов в реестре,
//! R — глубин пересчитано (не взято из памяти книги глубины), E — рёбер графа.
//! Включаемый: log.graph_timing = да И задан log.graph_timing_min_ms — пишется построение, у которого ожидание + build не
//! меньше порога (0 — каждое). Любая из двух не задана или log.graph_timing = нет — записи нет, замер ничего не стоит,
//! кроме двух чтений настроек на построение.
//! Кто вызывает: pools/graph_cache.rs (cached_as_of — после построения).
//! От чего зависит: ctx.rs (настройки, журнал), graph.rs (BuildStats), tokio (запись отдельной задачей).
//! Настройки: log.graph_timing, log.graph_timing_min_ms (вся система).

use std::time::Duration;

use serde_json::json;

use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::graph::BuildStats;

/// Замер одного построения.
pub struct Measure {
    /// Ожидание замка построения.
    pub wait: Duration,
    /// Сколько построений уже ждали замок, когда это встало в очередь.
    pub ahead: usize,
    /// Сколько ждут за ним в момент получения замка.
    pub behind: usize,
    /// Время build.
    pub build: Duration,
    pub stats: BuildStats,
    /// Рёбер в графе.
    pub edges: usize,
}

/// Порог записи (мс), если замер включён: log.graph_timing = да и задан log.graph_timing_min_ms. Иначе None. Ошибок нет.
fn threshold(ctx: &Ctx) -> Option<u64> {
    if !ctx.flag("log.graph_timing").unwrap_or(false) {
        return None;
    }
    ctx.int("log.graph_timing_min_ms").ok()
}

/// Запись замера в журнал (dex.graph_build), если замер включён и время не меньше порога. Вне среды tokio — без
/// записи. Ошибок нет.
pub fn report(ctx: &Ctx, m: &Measure) {
    let Some(min) = threshold(ctx) else {
        return;
    };
    let total = m.wait + m.build;
    if total < Duration::from_millis(min) {
        return;
    }
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let msg = format!(
        "построение графа: ожидание замка {:.1} мс (ждали при входе {}, ждут за ним {}), build {:.1} мс; \
         пулов N={}, глубин пересчитано R={}, рёбер E={}",
        ms(m.wait),
        m.ahead,
        m.behind,
        ms(m.build),
        m.stats.pools,
        m.stats.recomputed,
        m.edges
    );
    let data = json!({
        "wait_ms": ms(m.wait),
        "ahead": m.ahead,
        "behind": m.behind,
        "build_ms": ms(m.build),
        "pools": m.stats.pools,
        "recomputed": m.stats.recomputed,
        "edges": m.edges,
    });
    let ctx = ctx.clone();
    rt.spawn(async move {
        ctx.log(Level::Info, "dex.graph_build", msg, Some(data))
            .await;
    });
}
