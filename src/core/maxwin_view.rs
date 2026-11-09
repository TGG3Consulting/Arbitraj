//! Файл: core/maxwin_view.rs
//! Что делает: решение 245 (часть Б) — показ окна максимального объёма (uncapped_maxwin.rs) рядом с длительностью
//! наших сделок (deal_time.rs) и вывод «успели бы»: длительность окна ≥ медианы длительности сделки пары (или
//! биржи-источника) — «yes»; окно кончилось (расчёт, где оптимум не прошёл пороги) короче медианы — «no»; конец окна
//! неизвестен (идёт или расчётов не было дольше uncapped.maxwin_gap_ms) и известная часть короче медианы, медианы нет,
//! окна нет — «unknown» с причиной. Дополнение 09.11 (uncapped_watch.rs): окно держалось всё наблюдение
//! uncapped.maxwin_watch_ms — статус «held» (длительность «не меньше», на экране «2 м+»): ≥ медианы — «yes», медиана
//! длиннее наблюдения — «unknown». Берётся самое длинное окно записи (MaxWins.best). Общий для экрана «Упущено из-за
//! потолка», экрана «Окна» (API) и отчёта Excel «Упущено».
//! Кто вызывает: core/uncapped_view.rs, core/window_view.rs, core/uncapped_report.rs, report/missed_rows.rs.
//! От чего зависит: uncapped_maxwin.rs, deal_time.rs, ledger (сделки в памяти), settings.
//! Настройки: uncapped.maxwin_gap_ms (статус «оборвалось»), uncapped.deal_time_min_n (минимум сделок для медианы).

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::core::types::VenueId;
use crate::settings::read::At as SAt;

use super::deal_time::{DealTimes, Stat};
use super::engine::{Core, lk};
use super::uncapped_maxwin::MaxWins;
use super::uncapped_types::Brief;
use super::window_info::WindowInfo;

/// Всё для показа: время показа, настройки, длительности сделок учёта.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub now: DateTime<Utc>,
    pub gap: Result<u64, String>,
    pub min_n: Result<u64, String>,
    pub times: DealTimes,
}

/// Показ одного окна: статус (ended / held / open / cut / none), длительность, точная ли, медиана сделок, вывод и почему.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub status: &'static str,
    pub ms: Option<i64>,
    pub exact: bool,
    pub stat: Result<Stat, String>,
    pub would: &'static str,
    pub why: String,
}

impl View {
    /// JSON для API. Ошибок нет.
    pub fn json(&self) -> Value {
        let (stat, stat_err) = match &self.stat {
            Ok(s) => (json!(s), None),
            Err(e) => (Value::Null, Some(e.clone())),
        };
        json!({
            "status": self.status, "ms": self.ms, "exact": self.exact, "deal_time": stat,
            "deal_time_err": stat_err, "would": self.would, "why": self.why,
        })
    }
}

impl Core {
    /// Что делает: контекст показа (см. Ctx). Принимает: ничего. Возвращает: Ctx. Ошибок нет.
    pub fn maxwin_ctx(&self) -> Ctx {
        let int = |n: &str| {
            self.inner
                .deps
                .settings
                .integer(n, SAt::default())
                .map_err(|e| e.message)
        };
        let times = DealTimes::of(lk(&self.inner.deals).values());
        Ctx {
            now: Utc::now(),
            gap: int("uncapped.maxwin_gap_ms"),
            min_n: int("uncapped.deal_time_min_n"),
            times,
        }
    }
}

impl Ctx {
    /// Что делает: показ окна записи «без потолка» по её телу (JSON). Принимает: тело. Возвращает: JSON View; тело не
    /// читается как запись — Null. Ошибок нет.
    pub fn of_body(&self, body: &Value) -> Value {
        let Ok(b) = serde_json::from_value::<Brief>(body.clone()) else {
            return Value::Null;
        };
        let d = &b.direction;
        self.view(&d.buy_on, &d.sell_on, b.maxwin.as_ref()).json()
    }

    /// Что делает: показ окон максимального объёма направлений окна решения 125 (WindowInfo.maxwins). Принимает: сводку.
    /// Возвращает: [{buy_on, sell_on, wins, view}]. Ошибок нет.
    pub fn of_window(&self, w: &WindowInfo) -> Value {
        let v: Vec<Value> = w
            .maxwins
            .iter()
            .map(|m| {
                let view = self.view(&m.buy_on, &m.sell_on, Some(&m.wins)).json();
                json!({"buy_on": m.buy_on, "sell_on": m.sell_on, "wins": m.wins, "view": view})
            })
            .collect();
        Value::Array(v)
    }

    /// Что делает: показ окна направления (см. шапку). Принимает: площадку покупки и продажи, окна записи (None — нет).
    /// Возвращает: View. Ошибок нет.
    pub fn view(&self, buy: &VenueId, sell: &VenueId, w: Option<&MaxWins>) -> View {
        let stat = self
            .min_n
            .clone()
            .map_err(|e| format!("нет данных: {e}"))
            .and_then(|n| self.times.stat(buy, sell, n));
        let Some(b) = w.map(|m| &m.best) else {
            let why = match &self.gap {
                Ok(_) => "окна максимального объёма нет (запись до решения 245 или оптимум не проходил пороги)".into(),
                Err(e) => format!("окна максимального объёма не ведутся: {e}"),
            };
            return View {
                status: "none",
                ms: None,
                exact: false,
                stat,
                would: "unknown",
                why,
            };
        };
        let idle = |g: &u64| {
            (self.now - b.last_pass_at).num_milliseconds() > i64::try_from(*g).unwrap_or(i64::MAX)
        };
        let status = match b.end.as_deref() {
            Some("not_passing") => "ended",
            Some("watch") => "held",
            Some(_) => "cut",
            None if self.gap.as_ref().is_ok_and(idle) => "cut",
            None => "open",
        };
        let (ms, exact) = (b.ms(), status == "ended");
        let (would, why) = match &stat {
            Err(e) => ("unknown", e.clone()),
            Ok(s) if ms >= s.median_ms => (
                "yes",
                format!("окно {ms} мс ≥ медианы сделки {} мс", s.median_ms),
            ),
            Ok(s) if exact => (
                "no",
                format!("окно {ms} мс < медианы сделки {} мс", s.median_ms),
            ),
            Ok(s) => (
                "unknown",
                format!(
                    "окно не меньше {ms} мс, конец неизвестен ({}), медиана сделки {} мс",
                    match status {
                        "open" => "идёт",
                        "held" => "держалось всё время наблюдения uncapped.maxwin_watch_ms",
                        _ => "расчётов не было дольше uncapped.maxwin_gap_ms",
                    },
                    s.median_ms
                ),
            ),
        };
        View {
            status,
            ms: Some(ms),
            exact,
            stat,
            would,
            why,
        }
    }
}
