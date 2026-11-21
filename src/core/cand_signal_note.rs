//! Файл: core/cand_signal_note.rs
//! Что делает: решение 359 — в запись отказа кандидата (журнал core.candidate_rejected, поле data) добавляется поле
//! signal_note: сигнал, по которому шёл расчёт, и возраст цен обеих сторон. Только запись — поведение не меняется.
//! Зачем (разбор отказов 14–16.11, А1): у 6 из 18 отказов спред расчёта отрицательный, хотя сигнал требует
//! положительного; в записи не было ни спреда, ни объёма, ни времени сигнала — причина (цена ушла за время очереди,
//! котировка DEX из памяти, разные маршруты) оставалась гипотезой.
//! Что пишется:
//! - signal — строка «Живого экрана» направления в момент начала расчёта (последний пересчёт сигнала до расчёта: тот,
//!   что поставил в очередь, или более новый): время, состояние, спред %, объём сравнения, средние цены, комиссии %,
//!   время котировки каждой стороны, с какого момента спред выше порога, что запустило пересчёт, и возраст (мс) строки и
//!   котировок сторон на начало расчёта; строки нет — signal = null;
//! - calc — у отказа с расчётом: время расчёта, спред % средних цен расчёта, объём, время котировки сторон и их возраст
//!   (мс) на момент расчёта, мс от сигнала до расчёта.
//!
//! Кто вызывает: core/candidates.rs (calc_one — start в начале расчёта, data при отказе).
//! От чего зависит: core/engine.rs (spreads), core/signal_types.rs (SpreadView), core/costs_types.rs (Calc).
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::Direction;

use super::costs_types::Calc;
use super::engine::{Core, rd};
use super::signal_types::SpreadView;

/// Сигнал на начало расчёта кандидата.
/// Строка сигнала — в Box: Start живёт в будущем calc_one через весь расчёт (стек пути расчёта).
pub struct Start {
    view: Option<Box<SpreadView>>,
    at: DateTime<Utc>,
}

/// Что делает: снимок строки сигнала направления на начало расчёта. Принимает: ядро, направление. Ошибок нет.
pub fn start(core: &Core, d: &Direction) -> Start {
    Start {
        view: rd(&core.inner.spreads).get(d).cloned().map(Box::new),
        at: Utc::now(),
    }
}

/// Мс от `b` до `a`. Ошибок нет.
fn ms(a: DateTime<Utc>, b: DateTime<Utc>) -> i64 {
    (a - b).num_milliseconds()
}

/// Что делает: поле data записи отказа — как раньше (Calc лучшего расчёта или ничего) плюс signal_note (см. шапку).
/// Принимает: лучший расчёт (None — отказ без расчёта), снимок сигнала. Возвращает: JSON поля data. Ошибок нет.
pub fn data(best: Option<&Calc>, st: &Start) -> Option<Value> {
    let signal = st.view.as_ref().map(|v| {
        let p = v.prices.as_ref();
        json!({
            "at": v.at,
            "state": v.state,
            "spread_pct": v.spread_pct,
            "qty": p.map(|p| p.qty),
            "buy": p.map(|p| p.buy),
            "sell": p.map(|p| p.sell),
            "fees_pct": p.map(|p| p.fees_pct),
            "buy_quote_at": p.map(|p| p.buy_at),
            "sell_quote_at": p.map(|p| p.sell_at),
            "above_since": v.above_since,
            "trigger": v.trigger,
            "age_at_calc_start_ms": {
                "row": ms(st.at, v.at),
                "buy": p.map(|p| ms(st.at, p.buy_at)),
                "sell": p.map(|p| ms(st.at, p.sell_at)),
            },
        })
    });
    let calc = best.map(|c| {
        let spread = if c.buy_avg.is_zero() {
            None
        } else {
            Some((c.sell_avg - c.buy_avg) / c.buy_avg * Decimal::ONE_HUNDRED)
        };
        json!({
            "at": c.at,
            "spread_pct": spread,
            "qty": c.qty,
            "buy_quote_at": c.buy_quote.at,
            "sell_quote_at": c.sell_quote.at,
            "age_at_calc_ms": {
                "buy": ms(c.at, c.buy_quote.at),
                "sell": ms(c.at, c.sell_quote.at),
            },
            "since_signal_ms": st.view.as_ref().map(|v| ms(c.at, v.at)),
        })
    });
    let note = json!({ "calc_started_at": st.at, "signal": signal, "calc": calc });
    match best.and_then(|b| serde_json::to_value(b).ok()) {
        Some(Value::Object(mut m)) => {
            m.insert("signal_note".into(), note);
            Some(Value::Object(m))
        }
        Some(other) => Some(other),
        None => Some(json!({ "signal_note": note })),
    }
}
