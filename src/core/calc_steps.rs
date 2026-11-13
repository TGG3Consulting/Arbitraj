//! Файл: core/calc_steps.rs
//! Что делает: решение 265 — текст «Время по шагам» пересчёта сделки (profit::decide / decide_capped) по замеру этапов
//! (calc_report.rs): каждый этап (делистинг, закрытые входы, свежесть налогов, контекст направления, бюджет, подбор
//! объёма, финальная котировка) — мс; у подбора объёма — сколько расчётов объёма и в среднем мс на расчёт, части:
//! границы (бисекция), точки (изломы и сетка), уточнение, котировки сторон с id площадки; у финальной котировки —
//! котировка A, стоимость вывода, доля глубины B, котировка B, возврат USDT, курсы затрат, исполнимый объём у
//! исполнителя и «прочее» (худшие цены, пределы ордера, сборка затрат — то, что вне замеренных частей); у каждой
//! части — запросы к узлу BSC и агрегатору (calc_meter.rs), если были; «вне этапов» (цена покупки, параметры подбора,
//! пороги прибыли); HTTP-запросы к биржам за пересчёт (step_meter.rs) по именам. Части подбора объёма пересекаются
//! (границы и точки содержат расчёты объёма, а те — котировки), поэтому «прочее» у него не считается.
//! Кто вызывает: core/step_timing.rs.
//! От чего зависит: core/calc_report.rs, core/size_search.rs и core/size_memo.rs (имена частей подбора),
//! gateway/step_meter.rs.
//! Настройки: нет.

use crate::gateway::step_meter::Req;

use super::calc_report::{Report, Row};

/// Число мс до 0,1 без лишних нулей. Ошибок нет.
fn ms(v: f64) -> String {
    format!("{}", (v * 10.0).round() / 10.0)
}

/// Среднее мс до 0,01 (расчёт объёма бывает короче 0,1 мс). Принимает: сумму мс, сколько раз. Ошибок нет.
fn avg(sum: f64, n: u32) -> String {
    format!("{}", (sum / f64::from(n.max(1)) * 100.0).round() / 100.0)
}

/// Запросы строки к узлу и агрегатору: «узел BSC 3 запр. (очередь 1 мс, ответ 4 мс)». Ошибок нет.
fn row_calls(r: &Row) -> Vec<String> {
    let mut out = Vec::new();
    let one = |who: &str, n: u64, w: f64, s: f64| {
        format!("{who} {n} запр. (очередь {} мс, ответ {} мс)", ms(w), ms(s))
    };
    if r.node_calls > 0 {
        out.push(one("узел BSC", r.node_calls, r.node_wait_ms, r.node_ms));
    }
    if r.agg_calls > 0 {
        out.push(one("агрегатор", r.agg_calls, r.agg_wait_ms, r.agg_ms));
    }
    out
}

/// Строка этапа или части: «имя N мс ×k, в среднем M мс (узел BSC …)». Ошибок нет.
fn line(r: &Row) -> String {
    let mut s = format!("{} {} мс", r.name, ms(r.ms));
    if r.count > 1 {
        s.push_str(&format!(
            " ×{}, в среднем {} мс",
            r.count,
            avg(r.ms, r.count)
        ));
    }
    if r.open {
        s.push_str(" (не закончен)");
    }
    let c = row_calls(r);
    if !c.is_empty() {
        s.push_str(&format!(" ({})", c.join("; ")));
    }
    s
}

/// Этап с частями. У подбора объёма — число расчётов объёма и среднее на расчёт; у этапа без частей подбора —
/// «прочее» (время этапа вне замеренных частей). Ошибок нет.
fn stage(r: &Row) -> String {
    let size = [
        super::size_search::BOUNDS,
        super::size_search::POINTS,
        super::size_search::REFINE,
        super::size_memo::EVAL,
    ];
    let mut head = line(r);
    if let Some(e) = r.parts.iter().find(|p| p.name == super::size_memo::EVAL) {
        head.push_str(&format!(
            " — расчётов объёма {}, в среднем {} мс на расчёт",
            e.count,
            avg(e.ms, e.count)
        ));
    }
    if r.parts.is_empty() {
        return head;
    }
    let mut parts: Vec<String> = r.parts.iter().map(line).collect();
    if !r.parts.iter().any(|p| size.contains(&p.name.as_str())) {
        let inside: f64 = r.parts.iter().map(|p| p.ms).sum();
        parts.push(format!(
            "прочее (вне замеренных частей) {} мс",
            ms(r.ms - inside)
        ));
    }
    format!("{head} [{}]", parts.join("; "))
}

/// Текст этапов пересчёта (без заголовка): этапы; «вне этапов»; HTTP-запросы к биржам по именам; запросы к узлу и
/// агрегатору за весь пересчёт. Принимает: отчёт замера, HTTP-запросы. Ошибок нет.
pub fn text(r: &Report, reqs: &[Req]) -> String {
    let mut out: Vec<String> = r.stages.iter().map(stage).collect();
    let inside: f64 = r.stages.iter().map(|s| s.ms).sum();
    out.push(format!(
        "вне этапов (цена покупки, параметры подбора, пороги) {} мс",
        ms(r.total_ms - inside)
    ));
    out.push(http_sum(reqs));
    out.extend(calls(r));
    out.join("; ")
}

/// HTTP-запросы к биржам по именам в порядке первого появления: «HTTP-запросы к биржам: GET /api/v3/depth ×2 240 мс».
/// Нет — «HTTP-запросов к биржам нет». Ошибок нет.
fn http_sum(reqs: &[Req]) -> String {
    let mut by: Vec<(String, u32, u128)> = Vec::new();
    for q in reqs {
        match by.iter_mut().find(|b| b.0 == q.name) {
            Some(b) => {
                b.1 += 1;
                b.2 += q.ms;
            }
            None => by.push((q.name.clone(), 1, q.ms)),
        }
    }
    if by.is_empty() {
        return "HTTP-запросов к биржам нет".into();
    }
    let parts: Vec<String> = by
        .iter()
        .map(|(n, k, t)| format!("{n} ×{k} {t} мс"))
        .collect();
    format!("HTTP-запросы к биржам: {}", parts.join(", "))
}

/// Запросы к узлу BSC и агрегатору за весь замер; не было — «запросов к узлу BSC и агрегатору нет». Ошибок нет.
pub fn calls(r: &Report) -> Vec<String> {
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
    let c = row_calls(&total);
    if c.is_empty() {
        return vec!["запросов к узлу BSC и агрегатору нет".into()];
    }
    c.into_iter().map(|x| format!("всего {x}")).collect()
}
