//! Файл: venues/bsc_dex/quote/quote_timing_log.rs
//! Что делает: запись замера котировки DEX (решение 312) в журнал — kind dex.quote_timing: коротко в message, числами в
//! data (как dex.graph_build, pools/graph_timing.rs). Пишется, если всё время котировки (от входа в Quoting::priced /
//! быстрый путь fills.rs до конца) не меньше log.quote_timing_min_ms. Запись — отдельной задачей tokio (котировку не
//! задерживает; ctx.log пишет в SQLite через spawn_blocking и замок соединения). Вне среды tokio — без записи.
//! Поля data: mode (classic / graph — dex.router_mode), fast (быстрый путь fills.rs, решение 250), answered (быстрый путь
//! ответил — полная котировка не нужна), coin, side, qty, ok, error (код ошибки; ok и error — null, если итог не
//! известен), dropped (котировку бросил вызывающий — срок расчёта ядра), abandoned (счёт брошен по сроку —
//! quote/abandon.rs), cpu_max (dex.quote_cpu_max); total_ms — всё время; pre_ms (до входа в cpu::run; ошибка до работы —
//! до конца), run_ms (cpu::run целиком) = wait_ms (место dex.quote_cpu_max и старт потока spawn_blocking — вместе) +
//! work_ms (работа в потоке счёта) + back_ms (возврат итога из потока счёта); post_ms (после cpu::run); части
//! `<часть>_ms` (pre_decimals, pre_gas, post_approve, post_native, post_tip, fast_pool), pre_rest_ms / post_rest_ms
//! (остаток вне частей); stages_ms (сумма этапов верхнего уровня ≤ work_ms), rest_ms (work_ms − stages_ms — вне
//! замеренных этапов); по каждому этапу `<этап>_ms` и `<этап>_n`; счётчики paths_in, paths_ok, hops, best_evals,
//! split_steps, split_evals. Чего не было (работы, возврата) — null. У быстрого пути rest_ms — всё время вне частей.
//! Кто вызывает: quote/quote_timing_outer.rs (finish, Drop, hand).
//! От чего зависит: quote/quote_timing_outer.rs (Body, Outcome), quote/quote_timing.rs (Rec, ms), quote/quote_timing_stages.rs (имена),
//! ctx.rs (настройки, журнал), tokio.
//! Настройки: log.quote_timing_min_ms (порог — в Body); читает dex.router_mode, dex.quote_cpu_max [bsc_dex].

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::core::types::Side;
use crate::storage::journal::Level;

use super::quote_timing::{Rec, TOP, ms};
use super::quote_timing_outer::{Body, Outcome};
use super::quote_timing_stages::{COUNT_NAMES, PART_NAMES, Part, STAGE_NAMES};

/// Длительность от `a` до `b` в мс (оба есть) или null. Ошибок нет.
fn span(a: Option<Instant>, b: Option<Instant>) -> Option<f64> {
    Some(ms(b?.saturating_duration_since(a?)))
}

/// Пишет замер котировки в журнал, если всё время не меньше порога. Принимает: замер снаружи работы, замер работы (если
/// была), итог (если известен), брошена ли котировка вызывающим. Ошибок нет.
pub fn report(b: &Body, rec: Option<&Rec>, out: Option<&Outcome>, dropped: bool) {
    let end = Instant::now();
    let total = end.saturating_duration_since(b.t0);
    if total < Duration::from_millis(b.min_ms) {
        return;
    }
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let ctx = &b.ctx;
    let mode = match ctx.text("dex.router_mode").ok().as_deref() {
        Some("graph") => "graph",
        _ => "classic",
    };
    let (dir, side) = match b.meta.side {
        Side::Buy => ("покупка", "buy"),
        Side::Sell => ("продажа", "sell"),
    };
    let part = |p: Part| b.parts[p as usize] as f64 / 1e6;
    let tm = ms(total);
    // Границы: до работы — до входа в cpu::run (ошибка до работы — до конца); после — от возврата из cpu::run.
    let pre = if b.fast {
        None
    } else {
        span(Some(b.t0), b.cpu_in.or(Some(end)))
    };
    let post = span(b.cpu_out, Some(end));
    let work = rec.map(|r| ms(r.end.saturating_duration_since(r.work)));
    let stages = rec.map(|r| r.ns[..TOP].iter().map(|n| *n as f64 / 1e6).sum::<f64>());
    let pre_parts = part(Part::Decimals) + part(Part::Gas);
    let post_parts = part(Part::Approve) + part(Part::Native) + part(Part::Tip);
    let mut d = Map::new();
    d.insert("mode".into(), json!(mode));
    d.insert("fast".into(), json!(b.fast));
    d.insert("answered".into(), json!(out.and_then(|o| o.answered)));
    d.insert("coin".into(), json!(b.meta.ticker));
    d.insert("side".into(), json!(side));
    d.insert("qty".into(), json!(b.meta.qty.to_string()));
    d.insert("ok".into(), json!(out.map(|o| o.ok)));
    d.insert("error".into(), json!(out.and_then(|o| o.error.clone())));
    d.insert("dropped".into(), json!(dropped));
    d.insert("abandoned".into(), json!(rec.map(|r| r.abandoned)));
    d.insert("cpu_max".into(), json!(ctx.int("dex.quote_cpu_max").ok()));
    d.insert("total_ms".into(), json!(tm));
    d.insert("pre_ms".into(), json!(pre));
    d.insert("run_ms".into(), json!(span(b.cpu_in, b.cpu_out)));
    d.insert(
        "wait_ms".into(),
        json!(rec.map(|r| ms(r.work.saturating_duration_since(r.cpu_in)))),
    );
    d.insert("work_ms".into(), json!(work));
    d.insert(
        "back_ms".into(),
        json!(rec.and_then(|r| span(Some(r.end), b.cpu_out))),
    );
    d.insert("post_ms".into(), json!(post));
    for (i, (key, _)) in PART_NAMES.iter().enumerate() {
        d.insert(format!("{key}_ms"), json!(b.parts[i] as f64 / 1e6));
    }
    d.insert("pre_rest_ms".into(), json!(pre.map(|p| p - pre_parts)));
    d.insert("post_rest_ms".into(), json!(post.map(|p| p - post_parts)));
    d.insert("stages_ms".into(), json!(stages));
    let rest = match (b.fast, work, stages) {
        (true, _, _) => Some(tm - part(Part::Decimals) - part(Part::OnePool)),
        (false, Some(w), Some(s)) => Some(w - s),
        _ => None,
    };
    d.insert("rest_ms".into(), json!(rest));
    let (tick, qty) = (&b.meta.ticker, b.meta.qty);
    let kind = if b.fast {
        "быстрый путь"
    } else {
        mode
    };
    let mut msg = format!("котировка DEX {tick} {dir} {qty} ({kind}): всего {tm:.1} мс");
    if dropped {
        msg.push_str(" (брошена вызывающим)");
    }
    if let Some(p) = pre {
        let _ = write!(msg, "; до работы {p:.1}");
    }
    if let Some(r) = rec {
        let w = ms(r.work.saturating_duration_since(r.cpu_in));
        let _ = write!(
            msg,
            "; ожидание до старта {w:.1}; работа {:.1}",
            work.unwrap_or(0.0)
        );
    }
    if let Some(p) = post {
        let _ = write!(msg, "; после работы {p:.1}");
    }
    for (i, (_, label)) in PART_NAMES.iter().enumerate() {
        if b.parts[i] > 0 {
            let _ = write!(msg, "; {label} {:.1}", b.parts[i] as f64 / 1e6);
        }
    }
    if let Some(r) = rec {
        stages_into(r, &mut d, &mut msg);
        let _ = write!(msg, "; вне этапов {:.1}", rest.unwrap_or(0.0));
    }
    let ctx = ctx.clone();
    rt.spawn(async move {
        ctx.log(Level::Info, "dex.quote_timing", msg, Some(Value::Object(d)))
            .await;
    });
}

/// Этапы и счётчики работы — в data и message. Принимает: замер работы, data, message. Ошибок нет.
fn stages_into(r: &Rec, d: &mut Map<String, Value>, msg: &mut String) {
    for (i, (key, label)) in STAGE_NAMES.iter().enumerate() {
        let v = r.ns[i] as f64 / 1e6;
        d.insert(format!("{key}_ms"), json!(v));
        d.insert(format!("{key}_n"), json!(r.calls[i]));
        if r.calls[i] > 0 {
            let sub = if i >= TOP { "в т.ч. " } else { "" };
            let _ = write!(msg, "; {sub}{label} {v:.1} мс ×{}", r.calls[i]);
        }
    }
    for (i, key) in COUNT_NAMES.iter().enumerate() {
        d.insert((*key).into(), json!(r.counts[i]));
    }
    let [paths_in, paths_ok, hops, best, steps, evals] = r.counts;
    let _ = write!(
        msg,
        "; путей {paths_in} → допущено {paths_ok}, хопов {hops}; eval отбора {best}; сплит шагов {steps}, eval {evals}"
    );
}
