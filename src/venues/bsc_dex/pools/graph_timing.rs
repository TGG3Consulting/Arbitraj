//! Файл: venues/bsc_dex/pools/graph_timing.rs
//! Что делает: замер построения графа маршрутов (решение 301, часть д; задача t132) — одна запись журнала
//! `dex.graph_build` на построение: сколько построение ждало замок (одно построение за раз, pools/graph_cache.rs),
//! сколько ещё ждали вместе с ним при входе и за ним при получении замка, время build, N — пулов в реестре,
//! R — глубин пересчитано (не взято из памяти книги глубины), E — рёбер графа.
//! Включаемый: log.graph_timing = да И задан log.graph_timing_min_ms — пишется построение, у которого ожидание + build не
//! меньше порога (0 — каждое). Любая из двух не задана или log.graph_timing = нет — записи нет, замер ничего не стоит,
//! кроме двух чтений настроек на построение.
//! Решение 308: report_bg — та же запись dex.graph_build для фонового владельца графа (graph_owner.rs): вид (incr —
//! накопительный круг, full — полная пересборка со сверкой), длительность, пулов оценено, пар пересобрано, рёбер,
//! отставание в момент публикации (возраст самой старой учтённой записи реестра), сколько изменений уже ждут следующего
//! круга; у full — причина, пулов в реестре, отличий сверки. Тот же порог log.graph_timing / log.graph_timing_min_ms.
//! Кто вызывает: pools/graph_cache.rs (cached_as_of — после построения), pools/graph_owner.rs (report_bg).
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

/// Решение 308: замер круга фонового владельца графа.
pub struct BgMeasure {
    /// «incr» — накопительный круг, «full» — полная пересборка со сверкой.
    pub kind: &'static str,
    /// Причина полной пересборки.
    pub reason: Option<String>,
    pub ms: f64,
    pub evaluated: usize,
    pub pairs: usize,
    pub edges: usize,
    /// Отставание в момент публикации, мс.
    pub lag_ms: u64,
    /// Изменённых пулов ждут следующего круга.
    pub pending: usize,
    /// Пулов в реестре (полная пересборка).
    pub pools: Option<usize>,
    /// Отличий сверки (полная пересборка; None — сравнивать было не с чем).
    pub differences: Option<usize>,
}

/// Запись замера круга фонового владельца (dex.graph_build), если замер включён и длительность не меньше порога. Вне
/// среды tokio — без записи. Ошибок нет.
pub fn report_bg(ctx: &Ctx, m: &BgMeasure) {
    let Some(min) = threshold(ctx) else {
        return;
    };
    if m.ms < min as f64 {
        return;
    }
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let msg = format!(
        "граф (фон, {}): {:.1} мс; пулов оценено {}, пар пересобрано {}, рёбер {}; отставание при публикации {} мс, \
         ждут следующего круга {}{}{}{}",
        m.kind,
        m.ms,
        m.evaluated,
        m.pairs,
        m.edges,
        m.lag_ms,
        m.pending,
        m.reason
            .as_deref()
            .map(|r| format!("; причина: {r}"))
            .unwrap_or_default(),
        m.pools
            .map(|n| format!("; пулов в реестре {n}"))
            .unwrap_or_default(),
        m.differences
            .map(|n| format!("; отличий сверки {n}"))
            .unwrap_or_default(),
    );
    let data = json!({
        "kind": m.kind,
        "reason": m.reason,
        "ms": m.ms,
        "evaluated": m.evaluated,
        "pairs": m.pairs,
        "edges": m.edges,
        "lag_ms": m.lag_ms,
        "pending": m.pending,
        "pools": m.pools,
        "differences": m.differences,
    });
    let ctx = ctx.clone();
    rt.spawn(async move {
        ctx.log(Level::Info, "dex.graph_build", msg, Some(data))
            .await;
    });
}
