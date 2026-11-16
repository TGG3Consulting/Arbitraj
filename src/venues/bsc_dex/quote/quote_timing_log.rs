//! Файл: venues/bsc_dex/quote/quote_timing_log.rs
//! Что делает: запись замера этапов котировки DEX (решение 312) в журнал — kind dex.quote_timing: коротко в message,
//! числами в data (как dex.graph_build, pools/graph_timing.rs). Пишется, если ожидание до старта работы + работа не
//! меньше log.quote_timing_min_ms. Запись — отдельной задачей tokio (котировку не задерживает; ctx.log пишет в SQLite
//! через spawn_blocking и замок соединения). Вне среды tokio — без записи.
//! Поля data: mode (classic / graph — dex.router_mode), coin, side (buy / sell), qty, ok, error (код ошибки котировки),
//! abandoned (срок расчёта вышел — quote/abandon.rs), cpu_max (dex.quote_cpu_max, если задана), total_ms (от входа в
//! cpu::run до конца работы), wait_ms (ожидание до старта работы: место по dex.quote_cpu_max и старт потока
//! spawn_blocking — вместе, не по отдельности), work_ms, stages_ms (сумма этапов верхнего уровня ≤ work_ms);
//! по каждому этапу `<этап>_ms` и `<этап>_n` (сколько раз); счётчики paths_in, paths_ok, hops, best_evals,
//! split_steps, split_evals.
//! Кто вызывает: quote/quote_timing.rs (timed).
//! От чего зависит: quote/quote_timing.rs (Rec, Meta), quote/quote_timing_stages.rs (имена), quote/abandon.rs, ctx.rs (настройки, журнал), tokio.
//! Настройки: log.quote_timing_min_ms (порог — в Rec.start); читает dex.router_mode, dex.quote_cpu_max [bsc_dex].

use std::fmt::Write as _;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::core::types::{Quote, Side};
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::abandon;
use super::quote_timing::{Meta, Rec, TOP, ms};
use super::quote_timing_stages::{COUNT_NAMES, STAGE_NAMES};

/// Пишет замер котировки в журнал, если ожидание + работа не меньше порога. Принимает: окружение, что котировалось,
/// замер, итог котировки. Ошибок нет.
pub fn report(ctx: &Ctx, meta: &Meta, rec: &Rec, out: &GatewayResult<Quote>) {
    let total = rec.start.t0.elapsed();
    if total < Duration::from_millis(rec.start.min_ms) {
        return;
    }
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let work = rec.work.elapsed();
    let wait = total.saturating_sub(work);
    let mode = match ctx.text("dex.router_mode").ok().as_deref() {
        Some("graph") => "graph",
        _ => "classic",
    };
    let side = match meta.side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    };
    let stage_ms = |i: usize| rec.ns[i] as f64 / 1e6;
    let stages: f64 = (0..TOP).map(stage_ms).sum();
    let mut data = Map::new();
    data.insert("mode".into(), json!(mode));
    data.insert("coin".into(), json!(meta.ticker));
    data.insert("side".into(), json!(side));
    data.insert("qty".into(), json!(meta.qty.to_string()));
    data.insert("ok".into(), json!(out.is_ok()));
    data.insert(
        "error".into(),
        json!(out.as_ref().err().map(|e| format!("{:?}", e.code))),
    );
    data.insert("abandoned".into(), json!(abandon::is_set()));
    data.insert("cpu_max".into(), json!(ctx.int("dex.quote_cpu_max").ok()));
    data.insert("total_ms".into(), json!(ms(total)));
    data.insert("wait_ms".into(), json!(ms(wait)));
    data.insert("work_ms".into(), json!(ms(work)));
    data.insert("stages_ms".into(), json!(stages));
    let (tick, qty) = (&meta.ticker, meta.qty);
    let dir = if side == "buy" { "покупка" } else { "продажа" };
    let (tm, wm, km) = (ms(total), ms(wait), ms(work));
    let mut msg = format!(
        "котировка DEX {tick} {dir} {qty} ({mode}): всего {tm:.1} мс = ожидание до старта {wm:.1} + работа {km:.1}"
    );
    for (i, (key, label)) in STAGE_NAMES.iter().enumerate() {
        data.insert(format!("{key}_ms"), json!(stage_ms(i)));
        data.insert(format!("{key}_n"), json!(rec.calls[i]));
        if rec.calls[i] > 0 {
            let sub = if i >= TOP { "в т.ч. " } else { "" };
            let _ = write!(msg, "; {sub}{label} {:.1} мс ×{}", stage_ms(i), rec.calls[i]);
        }
    }
    for (i, key) in COUNT_NAMES.iter().enumerate() {
        data.insert((*key).into(), json!(rec.counts[i]));
    }
    let [paths_in, paths_ok, hops, best, steps, evals] = rec.counts;
    let _ = write!(
        msg,
        "; путей {paths_in} → допущено {paths_ok}, хопов {hops}; eval отбора {best}; сплит шагов {steps}, eval {evals}"
    );
    let ctx = ctx.clone();
    rt.spawn(async move {
        ctx.log(Level::Info, "dex.quote_timing", msg, Some(Value::Object(data)))
            .await;
    });
}
