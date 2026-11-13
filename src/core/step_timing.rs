//! Файл: core/step_timing.rs
//! Что делает: решение 265 — «Время по шагам» сделки от положительного расчёта до исполненной покупки: строка шагов
//! («Время по шагам (этап): шаг N мс; шаг N мс; …») для хронологии сделки и журнала core.precheck, и замер работы
//! (`measure`) — этапы расчёта (calc_timing.rs: части, сколько раз), запросы к узлу BSC и агрегатору
//! (gateway/calc_meter.rs) и каждый HTTP-запрос к бирже (gateway/step_meter.rs: метод, путь, начало, длительность,
//! итог). Шаги, которые идут внутри адаптера (пробный ордер, ордер, опросы статуса, запрос сделок и комиссии), ядро
//! видит только как HTTP-запросы общего обмена gateway/http.rs exchange_full — контракт шлюза не меняется; чего там
//! нет (Bybit — свой обмен; задачи адаптера через spawn), входит в шаг ядра целиком.
//! Кто вызывает: core/precheck.rs, core/executor.rs.
//! measure ставит замер этапов «подробный» (calc_timing.rs Timing::detailed): только в нём пишутся части,
//! добавленные решением 265; замер «Кандидатов» (решение 129) остаётся прежним.
//! От чего зависит: core/calc_timing.rs, core/calc_report.rs, core/calc_steps.rs (текст этапов расчёта),
//! gateway/calc_meter.rs, gateway/step_meter.rs.
//! Решение 265 (уточнение, Тигран 13.11): замер включаемый — log.step_timing = да; не задана или нет — ни часов, ни
//! обёрток, ни строк (Steps пуст, measure_if — работа как есть).
//! Настройки: log.step_timing (вся система; читает executor.rs один раз на сделку).

use std::future::Future;
use std::time::Instant;

use crate::gateway::calc_meter;
use crate::gateway::step_meter::{self, Req};

use super::calc_report::{self, Report};
use super::calc_timing::{self, Timing};

/// Имя настройки-включателя (решение 265, Тигран 13.11: замер по шагам — включаемый).
pub const SETTING: &str = "log.step_timing";

/// Включён ли замер по шагам: log.step_timing = да. Не задана или не читается — выключен (это не ошибка, журнала
/// нет). Читается один раз на сделку (executor.rs). Принимает: хранилище настроек. Ошибок нет.
pub fn enabled(s: &crate::settings::store::Store) -> bool {
    s.flag(SETTING, crate::settings::read::At::default())
        .unwrap_or(false)
}

/// Шаги одного этапа сделки: (что, мс) по порядку. Выключен (log.step_timing) — ничего не замеряет и не хранит.
#[derive(Debug, Default, Clone)]
pub struct Steps {
    on: bool,
    list: Vec<(String, i128)>,
}

impl Steps {
    /// Новый список. Принимает: включён ли замер. Ошибок нет.
    pub fn new(on: bool) -> Self {
        Steps {
            on,
            list: Vec::new(),
        }
    }

    /// Начало шага: сейчас, если замер включён; иначе None (часы не читаются). Ошибок нет.
    pub fn at(&self) -> Option<Instant> {
        self.on.then(Instant::now)
    }

    /// Запоминает шаг от `from` до сейчас; from None (замер выключен) — ничего, имя не собирается.
    /// Принимает: начало, имя (замыкание). Ошибок нет.
    pub fn mark(&mut self, from: Option<Instant>, what: impl FnOnce() -> String) {
        if let Some(f) = from {
            self.list.push((what(), f.elapsed().as_millis() as i128));
        }
    }

    /// Запоминает шаг с готовым числом мс (разница часов UTC), если замер включён. Ошибок нет.
    pub fn add(&mut self, what: impl FnOnce() -> (String, i128)) {
        if self.on {
            self.list.push(what());
        }
    }

    /// «шаг N мс; шаг N мс» (без заголовка). Ошибок нет.
    pub fn list(&self) -> String {
        let parts: Vec<String> = self
            .list
            .iter()
            .map(|(w, ms)| format!("{w} {ms} мс"))
            .collect();
        parts.join("; ")
    }

    /// «Время по шагам (этап): шаг N мс; …»; замер выключен — None. Принимает: название этапа. Ошибок нет.
    pub fn text(&self, stage: &str) -> Option<String> {
        self.on
            .then(|| format!("{HEAD} ({stage}): {}", self.list()))
    }
}

/// Строка «Время по шагам» в хронологию сделки, если она есть (None — замер выключен). Ошибок нет.
pub fn note(deal: &mut super::deal::Deal, line: Option<String>) {
    if let Some(l) = line {
        deal.note(l);
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
    let t = Timing::detailed(t0);
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

/// Как measure, но только если замер включён (on); выключен — работа без обёрток замера, замера нет.
/// Принимает: включён ли, работу. Возвращает: результат и замер (None — выключен). Ошибок нет.
pub async fn measure_if<F: Future>(on: bool, work: F) -> (F::Output, Option<Measured>) {
    if !on {
        return (work.await, None);
    }
    let (out, m) = measure(work).await;
    (out, Some(m))
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
