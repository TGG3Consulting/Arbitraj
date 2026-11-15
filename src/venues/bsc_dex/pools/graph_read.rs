//! Файл: venues/bsc_dex/pools/graph_read.rs
//! Что делает: решение 308 (задача t137) — сторона ЧИТАТЕЛЯ графа маршрутов в режиме dex.graph_background = да:
//! котировка (route_cache.rs), поиск циклов (cycle_scan.rs) берут последний опубликованный снимок (graph_snap.rs) — без
//! замка построения и без ожидания; сами граф не строят никогда (И1 проекта docs/notes/graph-snapshot-design-2026-11-16.md).
//! Ступени (п. 7 проекта; принцип Тиграна 16.11 — маршрут должен быть, отказ — последний рубеж):
//! - А: снимок на текущих настройках, отставание ≤ dex.graph_max_lag_ms (или предел не задан) — отдаётся;
//! - Б: снимок на прежних настройках графа (идёт полная пересборка после их смены) или догоняющая полная сборка /
//!   доводка (карты нет после перезапуска, доводка не даёт кругам идти) — отдаётся и отставшим не считается (цена всё
//!   равно по свежему состоянию: гидратация route_cache::fresh), счётчик и запись журнала на переход (graph_alarm.rs);
//! - В: снимка нет, он для другого USDT/decimals, или (вне ступени Б) отставание > dex.graph_max_lag_ms — ошибка,
//!   котировка идёт по классике (graph_route::merge). Текст ошибки начинается с QUIET: graph_route::failed такие ошибки
//!   в журнал не пишет (иначе запись на каждую котировку) — их журнал без спама ведёт graph_alarm.rs.
//!
//! Отставание, которое увидел расчёт, — в счётчики экрана (graph_stats.rs reader_lag; ревью №2 C).
//! Кто вызывает: graph_cache.rs (cached), route_cache.rs и quote/cycle_scan.rs (active — гидратация), graph_owner.rs и
//! graph_stats.rs, quote/graph_route.rs (QUIET).
//! От чего зависит: graph_snap.rs, graph_cache.rs (graph_settings), graph_alarm.rs, registry.rs, tokens.rs, ctx.rs.
//! Настройки: dex.graph_background, dex.graph_max_lag_ms [bsc_dex] (обе необязательные).

use std::sync::Arc;
use std::time::Duration;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::graph::Graph;
use super::graph_alarm;
use super::graph_cache::graph_settings;
use super::registry::Registry;

/// Начало текста ошибок ступени В: такие ошибки журналирует graph_alarm.rs, а не каждая котировка.
pub const QUIET: &str = "граф маршрутов (фон): ";

/// Включён ли режим «граф без ожидания» (dex.graph_background = да). Не задана — нет (поведение t132). Ошибок нет.
pub fn background(ctx: &Ctx) -> bool {
    ctx.flag("dex.graph_background").unwrap_or(false)
}

/// Режим включён И у реестра есть фоновый владелец (у замороженного реестра решения 217 и в тестах без подписки на рынок
/// владельца нет — прежний путь t132). Ошибок нет.
pub fn active(ctx: &Ctx, reg: &Registry) -> bool {
    background(ctx) && reg.graph_cache().bg().started()
}

/// Предел отставания снимка dex.graph_max_lag_ms. None — не задан (предохранителя нет). Ошибок нет.
pub fn max_lag(ctx: &Ctx) -> Option<Duration> {
    ctx.int("dex.graph_max_lag_ms")
        .ok()
        .map(Duration::from_millis)
}

/// Ошибка ступени В (код StaleData, текст с QUIET). Ошибок нет.
fn quiet(ctx: &Ctx, text: String) -> GatewayError {
    ctx.err(ErrorCode::StaleData, format!("{QUIET}{text}"))
}

/// Повторное ревью (д): граф для расчёта и признак «гидратировать» — режим читается ОДИН раз: включён и есть владелец —
/// снимок (current) и true; иначе — путь t132 (graph_cache::cached_as_of по текущей версии реестра) и false. Так снимок
/// без гидратации невозможен (раньше режим читался отдельно в cached и при гидратации). Принимает: окружение, реестр,
/// токены, знаки USDT. Ошибки: как у current / cached_as_of.
pub fn graph_for(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt_dec: u8,
) -> GatewayResult<(Arc<Graph>, bool)> {
    if active(ctx, reg) {
        return current(ctx, reg, tokens, usdt_dec).map(|g| (g, true));
    }
    super::graph_cache::cached_as_of(ctx, reg, tokens, usdt_dec, reg.version()).map(|g| (g, false))
}

/// Граф для расчёта — последний опубликованный снимок (ступени — см. шапку). Принимает: окружение, реестр, токены,
/// знаки USDT. Возвращает: граф снимка. Ошибки: StaleData (текст с QUIET) — снимка нет, другой USDT, предохранитель;
/// NotConfigured — USDT не известен.
pub fn current(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt_dec: u8,
) -> GatewayResult<Arc<Graph>> {
    let bg = reg.graph_cache().bg();
    let Some(s) = bg.load() else {
        bg.changes.poke();
        let why = bg
            .why_none()
            .unwrap_or_else(|| "снимок ещё собирается фоновой задачей".into());
        return Err(quiet(
            ctx,
            format!("снимка нет ({why}) — котировка по классике"),
        ));
    };
    let usdt = tokens.usdt(ctx)?;
    if s.basis.usdt != usdt || s.basis.usdt_dec != usdt_dec {
        bg.changes.poke();
        return Err(quiet(
            ctx,
            "снимок собран для другого USDT — ждёт пересборки, котировка по классике".into(),
        ));
    }
    let lag = bg.changes.lag();
    let lag_ms = lag.map_or(0, |l| u64::try_from(l.as_millis()).unwrap_or(u64::MAX));
    // Ступень Б (ревью №2 A, B): прежние настройки графа или догоняющая полная сборка / доводка — действующий снимок
    // отдаётся и отставшим не считается (цена — по свежему состоянию, устаревшая топология — не «вслепую»).
    let stage = if s.basis.settings != graph_settings(ctx) {
        bg.changes.poke();
        Some("настройки графа сменились, новый снимок собирается")
    } else if bg.catching_up() {
        Some("идёт полная пересборка без накопительной карты или её доводка")
    } else {
        None
    };
    graph_alarm::stage_b(ctx, bg, stage);
    // Третье ревью п. 1в: ступень Б просрочена и после перезапуска — льготы нет, снимок проверяется предохранителем.
    if stage.is_some() && !bg.stage_b_overdue() {
        // Повторное ревью п. 6: отставание в ступени Б — отдельной метрикой (предохранитель здесь не действует).
        bg.stats.reader_lag_b(lag_ms);
        graph_alarm::fresh_again(ctx, bg);
        return Ok(s.graph.clone());
    }
    // Ревью №2 (C), повторное ревью п. 6: отставание, которое видит расчёт в ступени А, — метрика подбора
    // dex.graph_max_lag_ms (только там, где предохранитель действует).
    bg.stats.reader_lag(lag_ms);
    if let Some(limit) = max_lag(ctx)
        && let Some(lag) = lag
        && lag > limit
    {
        graph_alarm::fuse(ctx, bg, lag, limit, s.version, reg.version());
        return Err(quiet(
            ctx,
            format!(
                "снимок отстал на {} мс (предел dex.graph_max_lag_ms = {} мс) — котировка по классике",
                lag.as_millis(),
                limit.as_millis()
            ),
        ));
    }
    graph_alarm::fresh_again(ctx, bg);
    Ok(s.graph.clone())
}
