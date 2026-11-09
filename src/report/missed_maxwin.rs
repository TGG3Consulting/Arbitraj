//! Файл: report/missed_maxwin.rs
//! Что делает: решение 245 (часть Б) — столбцы окна максимального объёма у строки эпизода листа «Кандидаты» отчёта
//! «Упущено»: начало, конец (или почему неизвестен), длительность и точная ли она, вложить и прибыль на начале и в пике,
//! сколько окон у эпизода, медиана и p75 длительности сделки (пара или биржа-источник, сколько сделок; дополнение 245
//! п. 2 — от решения о сделке, сколько из них от открытия), «успели бы».
//! Значения — тот же показ, что на экране (core/maxwin_view.rs View, самое длинное окно эпизода MaxWins.best).
//! Кто вызывает: report/missed_rows.rs (candidates), report/missed_cands.rs (лист).
//! От чего зависит: core/maxwin_view.rs, core/uncapped_maxwin.rs, missed_rows.rs (num, when).
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::maxwin_view::View;
use crate::core::uncapped_maxwin::MaxWins;

use super::missed_rows::{num, when};

/// Столбцы окна максимального объёма строки эпизода. Окна нет — пустые числа и причина в «успели бы».
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MaxWinCols {
    pub start: String,
    pub end: String,
    pub secs: Option<f64>,
    /// «точно» / «не меньше (идёт)» / «не меньше (держалось всё наблюдение)» / «не меньше (расчётов не было)»; окна
    /// нет — пусто.
    pub kind: String,
    pub start_invested: Option<f64>,
    pub start_profit: Option<f64>,
    pub peak_invested: Option<f64>,
    pub peak_profit: Option<f64>,
    pub n: Option<f64>,
    pub median_s: Option<f64>,
    pub p75_s: Option<f64>,
    /// «пара, N сделок» / «биржа-источник, N сделок» / причина «нет данных».
    pub deal_by: String,
    /// «да» / «нет» / «нет данных: …».
    pub would: String,
}

/// Секунды из мс (3 знака). Ошибок нет.
fn secs(ms: i64) -> f64 {
    num(Decimal::new(ms, 3), 3)
}

/// Что делает: столбцы по показу и окнам записи. Принимает: показ (maxwin_view), окна эпизода (None — нет).
/// Возвращает: MaxWinCols. Ошибок нет.
pub fn cols(v: &View, w: Option<&MaxWins>) -> MaxWinCols {
    let mut c = MaxWinCols::default();
    match &v.stat {
        Ok(s) => {
            c.median_s = Some(secs(s.median_ms));
            c.p75_s = Some(secs(s.p75_ms));
            let by = if s.by == "pair" {
                "пара"
            } else {
                "биржа-источник"
            };
            c.deal_by = format!("{by}, {} сделок", s.n);
            // Дополнение 245 п. 2: длительность — от решения о сделке; у старых сделок — от открытия.
            if s.from_open > 0 {
                c.deal_by += &format!(
                    ", из них {} — от открытия (момента решения в учёте нет)",
                    s.from_open
                );
            }
        }
        Err(e) => c.deal_by = e.clone(),
    }
    c.would = match v.would {
        "yes" => "да".into(),
        "no" => "нет".into(),
        _ => format!("нет данных: {}", v.why),
    };
    let Some(m) = w else { return c };
    let b = &m.best;
    c.start = when(b.start_at);
    c.end = match (v.status, b.end_at) {
        ("ended", Some(e)) => when(e),
        ("open", _) => "идёт".into(),
        // Дополнение 09.11: держалось всё наблюдение uncapped.maxwin_watch_ms.
        ("held", _) => format!(
            "держалось всё наблюдение ({} с+), последний выгодный расчёт {}",
            b.watch_ms.map_or(0, |w| w / 1000),
            when(b.last_pass_at)
        ),
        _ => format!(
            "неизвестен: расчётов не было после {}",
            when(b.last_pass_at)
        ),
    };
    c.secs = v.ms.map(secs);
    c.kind = match v.status {
        "ended" => "точно",
        "open" => "не меньше (идёт)",
        "held" => "не меньше (держалось всё наблюдение)",
        _ => "не меньше (расчётов не было)",
    }
    .into();
    c.start_invested = Some(num(b.start.invested, 2));
    c.start_profit = Some(num(b.start.profit, 4));
    c.peak_invested = Some(num(b.peak.invested, 2));
    c.peak_profit = Some(num(b.peak.profit, 4));
    c.n = Some(m.n as f64);
    c
}
