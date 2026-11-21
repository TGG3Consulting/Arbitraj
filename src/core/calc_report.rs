//! Файл: core/calc_report.rs
//! Что делает: отчёт по замеру этапов расчёта кандидата (решение 129, calc_timing.rs) — поле `timing` строки
//! «Кандидатов» (видно на «Живом экране») и текст причины отказа по сроку profit.calc_timeout_ms: на каком этапе стоит
//! расчёт, сколько он там идёт, сколько длились прошедшие этапы и сколько запросов к узлу BSC / агрегатору они сделали
//! (ожидание очереди и время ответа отдельно — видно, ждал ли расчёт узел или считал сам). Ядро о площадках не знает:
//! имена этапов несут id площадки, источник запросов — «узел» или «агрегатор».
//! Кто вызывает: core/candidates.rs.
//! От чего зависит: core/calc_timing.rs, gateway/calc_meter.rs.
//! Настройки: нет (срок profit.calc_timeout_ms читает candidates.rs и передаёт числом).

use serde::Serialize;

use super::calc_timing::{Agg, Timing};

/// Этап или его часть в отчёте.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub name: String,
    pub ms: f64,
    pub count: u32,
    /// Этап ещё идёт (расчёт стоит на нём): ms — сколько он идёт к моменту отчёта.
    pub open: bool,
    pub node_calls: u64,
    pub node_wait_ms: f64,
    pub node_ms: f64,
    pub agg_calls: u64,
    pub agg_wait_ms: f64,
    pub agg_ms: f64,
    pub parts: Vec<Row>,
}

/// Отчёт по расчёту.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// От старта расчёта до отчёта.
    pub total_ms: f64,
    /// От постановки в очередь до старта расчёта (задача ждала планировщик tokio).
    pub queue_ms: f64,
    pub node_calls: u64,
    pub node_wait_ms: f64,
    pub node_ms: f64,
    pub agg_calls: u64,
    pub agg_wait_ms: f64,
    pub agg_ms: f64,
    pub stages: Vec<Row>,
    /// Этап и часть, на которых расчёт стоит (у отчёта по сроку); None — расчёт закончен или стоит между этапами.
    pub stuck: Option<(String, Option<String>)>,
}

/// Миллисекунды из микросекунд, до 0,1. Ошибок нет.
fn ms(us: u64) -> f64 {
    (us as f64 / 100.0).round() / 10.0
}

/// Строка отчёта из суммы этапа. Ошибок нет.
fn row(a: &Agg) -> Row {
    Row {
        name: a.name.clone(),
        ms: ms(a.us),
        count: a.count,
        open: false,
        node_calls: a.node.calls,
        node_wait_ms: ms(a.node.wait_us),
        node_ms: ms(a.node.send_us),
        agg_calls: a.agg.calls,
        agg_wait_ms: ms(a.agg.wait_us),
        agg_ms: ms(a.agg.send_us),
        parts: a.parts.iter().map(row).collect(),
    }
}

/// Отчёт на сейчас: закрытые этапы и идущие (с временем «уже идёт»). Принимает: замер. Ошибок нет.
pub fn report(t: &Timing) -> Report {
    let v = t.view();
    let mut stages: Vec<Row> = v.done.iter().map(row).collect();
    let (node, agg) = t.meter.snapshot();
    let mut stuck = None;
    for o in &v.open {
        let now = o.at.elapsed().as_micros() as u64;
        let (n1, a1) = t.meter.snapshot();
        let mut r = row(&Agg {
            name: o.name.clone(),
            us: now,
            count: 1,
            node: n1.since(o.node0),
            agg: a1.since(o.agg0),
            parts: Vec::new(),
        });
        r.open = true;
        match &o.parent {
            None => {
                stuck.get_or_insert((o.name.clone(), None));
                match stages.iter_mut().find(|s| s.name == o.name) {
                    // Части уже есть — оставляем их, время этапа — «уже идёт».
                    Some(s) => {
                        s.ms = r.ms;
                        s.open = true;
                    }
                    None => stages.push(r),
                }
            }
            Some(p) => {
                stuck = Some((p.clone(), Some(o.name.clone())));
                if let Some(s) = stages.iter_mut().find(|s| &s.name == p) {
                    s.parts.push(r);
                }
            }
        }
    }
    Report {
        total_ms: ms(t.started.elapsed().as_micros() as u64),
        queue_ms: ms(t.started.duration_since(t.queued_at).as_micros() as u64),
        node_calls: node.calls,
        node_wait_ms: ms(node.wait_us),
        node_ms: ms(node.send_us),
        agg_calls: agg.calls,
        agg_wait_ms: ms(agg.wait_us),
        agg_ms: ms(agg.send_us),
        stages,
        stuck,
    }
}

/// Запросы строки текстом: «узел 3 запр., очередь 1 мс, ответ 4 мс; агрегатор …»; запросов нет — пусто. Ошибок нет.
fn calls(r: &Row) -> String {
    let mut out = Vec::new();
    let part = |who: &str, n: u64, w: f64, s: f64| {
        format!("{who} {n} запр., очередь {w} мс, ответ {s} мс")
    };
    if r.node_calls > 0 {
        out.push(part("узел", r.node_calls, r.node_wait_ms, r.node_ms));
    }
    if r.agg_calls > 0 {
        out.push(part("агрегатор", r.agg_calls, r.agg_wait_ms, r.agg_ms));
    }
    if out.is_empty() {
        String::new()
    } else {
        format!(" ({})", out.join("; "))
    }
}

/// Строка этапа: «имя 12 мс ×3 (узел …)». Ошибок нет.
fn line(r: &Row) -> String {
    let times = if r.count > 1 {
        format!(" ×{}", r.count)
    } else {
        String::new()
    };
    format!("{} {} мс{times}{}", r.name, r.ms, calls(r))
}

/// Причина отказа по сроку и ключ повтора (без чисел — одинаковая причина не пишется в журнал повторно).
/// Принимает: срок, отчёт. Возвращает: (причина, ключ). Ошибок нет.
pub fn timeout_reason(limit_ms: u64, r: &Report) -> (String, String) {
    let head = format!("расчёт не уложился в срок {limit_ms} мс");
    let (at, key) = match &r.stuck {
        Some((s, Some(p))) => {
            let ms = r
                .stages
                .iter()
                .find(|x| &x.name == s)
                .and_then(|x| x.parts.iter().find(|y| y.open && &y.name == p))
                .map(line)
                .unwrap_or_else(|| p.clone());
            (
                format!("стоит на этапе «{s}» → {ms}"),
                format!("{head}: этап «{s}» → «{p}»"),
            )
        }
        Some((s, None)) => (
            format!("стоит на этапе «{s}» (вне замеренных частей)"),
            format!("{head}: этап «{s}»"),
        ),
        None => (
            "стоит вне замеренных этапов".to_string(),
            format!("{head}: вне этапов"),
        ),
    };
    let stages: Vec<String> = r
        .stages
        .iter()
        .map(|s| {
            let parts: Vec<String> = s.parts.iter().map(line).collect();
            let inner = if parts.is_empty() {
                String::new()
            } else {
                format!(" [{}]", parts.join(", "))
            };
            format!("{}{inner}", line(s))
        })
        .collect();
    let total = Row {
        name: String::new(),
        ms: 0.0,
        count: 0,
        open: false,
        node_calls: r.node_calls,
        node_wait_ms: r.node_wait_ms,
        node_ms: r.node_ms,
        agg_calls: r.agg_calls,
        agg_wait_ms: r.agg_wait_ms,
        agg_ms: r.agg_ms,
        parts: Vec::new(),
    };
    let reason = format!(
        "{head}: {at}; очередь до старта {} мс; этапы: {}; запросы за расчёт:{}",
        r.queue_ms,
        stages.join("; "),
        match calls(&total).as_str() {
            "" => " нет".to_string(),
            c => c.to_string(),
        }
    );
    (reason, key)
}
