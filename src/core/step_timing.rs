//! Файл: core/step_timing.rs
//! Что делает: решение 265 — «Время по шагам» сделки от положительного расчёта до исполненной покупки: строка шагов
//! («Время по шагам (этап): шаг N мс; шаг N мс; …») для хронологии сделки и журнала core.precheck, и замер работы
//! (`measure`) — этапы расчёта (calc_timing.rs: части, сколько раз), запросы к узлу BSC и агрегатору
//! (gateway/calc_meter.rs) и каждый HTTP-запрос к бирже (gateway/step_meter.rs: метод, путь, начало, длительность,
//! итог). Шаги, которые идут внутри адаптера (пробный ордер, ордер, опросы статуса, запрос сделок и комиссии), ядро
//! видит только как HTTP-запросы общего обмена gateway/http.rs exchange_full — контракт шлюза не меняется; чего там
//! нет (Bybit — свой обмен; задачи адаптера через spawn), входит в шаг ядра целиком.
//! Кто вызывает: core/precheck.rs, core/executor.rs.
//! От чего зависит: core/calc_timing.rs, core/calc_report.rs, core/calc_steps.rs (текст этапов расчёта),
//! gateway/calc_meter.rs, gateway/step_meter.rs.
//! Настройки: нет — замер всегда (два чтения часов на шаг; дёшево против запросов к площадкам).

use std::future::Future;
use std::time::Instant;

use crate::gateway::calc_meter;
use crate::gateway::step_meter::{self, Req};

use super::calc_report::{self, Report};
use super::calc_timing::{self, Timing};

/// Шаги одного этапа сделки: (что, мс) по порядку.
#[derive(Debug, Default, Clone)]
pub struct Steps(pub Vec<(String, i128)>);

impl Steps {
    /// Запоминает шаг от момента `from` до сейчас. Принимает: название, начало. Ошибок нет.
    pub fn mark(&mut self, what: impl Into<String>, from: Instant) {
        self.0
            .push((what.into(), from.elapsed().as_millis() as i128));
    }

    /// Запоминает шаг с готовым числом мс (разница часов UTC). Ошибок нет.
    pub fn add(&mut self, what: impl Into<String>, ms: i128) {
        self.0.push((what.into(), ms));
    }

    /// «шаг N мс; шаг N мс» (без заголовка). Ошибок нет.
    pub fn list(&self) -> String {
        let parts: Vec<String> = self
            .0
            .iter()
            .map(|(w, ms)| format!("{w} {ms} мс"))
            .collect();
        parts.join("; ")
    }

    /// «Время по шагам (этап): шаг N мс; …». Принимает: название этапа. Ошибок нет.
    pub fn text(&self, stage: &str) -> String {
        format!("{HEAD} ({stage}): {}", self.list())
    }
}

/// Начало каждой строки времени (его же ищут тесты).
pub const HEAD: &str = "Время по шагам";

/// Замер работы: этапы расчёта, запросы к узлу и агрегатору, HTTP-запросы к биржам, общее время.
pub struct Measured {
    pub report: Report,
    pub reqs: Vec<Req>,
    /// Всё время работы, мс (до 0,1).
    pub ms: f64,
}

/// Выполняет работу с замером (этапы calc_timing, счётчик calc_meter, журнал HTTP step_meter — все на эту задачу).
/// Принимает: работу. Возвращает: её результат и замер. Ошибок нет.
pub async fn measure<F: Future>(work: F) -> (F::Output, Measured) {
    let t0 = Instant::now();
    let t = Timing::new(t0);
    let rec = step_meter::Rec::new();
    let out = calc_meter::scope(
        t.meter.clone(),
        calc_timing::scope(t.clone(), step_meter::scope(rec.clone(), work)),
    )
    .await;
    let m = Measured {
        report: calc_report::report(&t),
        reqs: rec.take(),
        ms: (t0.elapsed().as_micros() as f64 / 100.0).round() / 10.0,
    };
    (out, m)
}

impl Measured {
    /// Строка этапов расчёта: «Время по шагам (этап, всего N мс): …» (calc_steps.rs). Ошибок нет.
    pub fn calc_text(&self, stage: &str) -> String {
        format!(
            "{HEAD} ({stage}, всего {} мс): {}",
            self.ms,
            super::calc_steps::text(&self.report, &self.reqs)
        )
    }

    /// Строка запросов шага по одному: «Время по шагам (этап, всего N мс): POST /api/v3/order/test 85 мс (с +0 мс,
    /// код 200); …; узел BSC …; агрегатор …». Запросов нет — так и сказано. Ошибок нет.
    pub fn reqs_text(&self, stage: &str) -> String {
        let mut parts: Vec<String> = self
            .reqs
            .iter()
            .map(|r| format!("{} {} мс (с +{} мс, {})", r.name, r.ms, r.at_ms, r.outcome))
            .collect();
        if parts.is_empty() {
            parts.push("HTTP-запросов к биржам нет".into());
        }
        parts.extend(super::calc_steps::calls(&self.report));
        format!(
            "{HEAD} ({stage}, всего {} мс): {}",
            self.ms,
            parts.join("; ")
        )
    }
}
