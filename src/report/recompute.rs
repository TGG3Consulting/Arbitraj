//! Файл: report/recompute.rs
//! Что делает: решение 218 — отчёт пересчёта старых записей «без потолка» по окнам (src/bin/uncapped_recompute.rs):
//! CSV (строка на сделку: сделка, направление, группа, было и стало упущено, потолок было и стало, источник, причина,
//! если не пересчитано; разделитель «;», UTF-8 с BOM — открывается Excel) и текст итогов по группам для stdout
//! (биржа ↔ биржа, биржа → DEX, не пересчитано — по видам причин).
//! Кто вызывает: src/bin/uncapped_recompute.rs, тесты (tests/uncapped_recompute.rs).
//! От чего зависит: core/uncapped_recompute.rs (Line, группы).
//! Настройки: нет.

use std::collections::BTreeMap;

use rust_decimal::Decimal;

use crate::core::uncapped_recompute::{G_CEX, G_DEX, G_SKIP, Line};

/// Что делает: CSV отчёта. Принимает: строки. Возвращает: текст файла (с BOM). Ошибок нет.
pub fn csv(lines: &[Line]) -> String {
    let mut out = String::from("\u{feff}");
    out.push_str(
        "сделка;направление;группа;было упущено USDT;стало упущено USDT;потолок было USDT;потолок стало USDT;источник;причина\n",
    );
    for l in lines {
        let cells = [
            l.deal_id.clone(),
            l.direction.clone(),
            l.group.to_string(),
            num(l.was_missed),
            num(l.now_missed),
            num(l.was_cap),
            num(l.now_cap),
            l.source.clone(),
            l.reason.clone().unwrap_or_default(),
        ];
        let row: Vec<String> = cells.iter().map(|c| cell(c)).collect();
        out.push_str(&row.join(";"));
        out.push('\n');
    }
    out
}

/// Что делает: итоги по группам текстом (stdout). Принимает: строки. Возвращает: текст. Ошибок нет.
pub fn summary(lines: &[Line]) -> String {
    let mut out = String::new();
    for g in [G_CEX, G_DEX] {
        let ls: Vec<&Line> = lines.iter().filter(|l| l.group == g).collect();
        let sum =
            |f: fn(&Line) -> Option<Decimal>| -> Decimal { ls.iter().filter_map(|l| f(l)).sum() };
        out.push_str(&format!(
            "{g}: пересчитано {}; упущено было {} → стало {} USDT; потолок было {} → стало {} USDT\n",
            ls.len(),
            sum(|l| l.was_missed).round_dp(2),
            sum(|l| l.now_missed).round_dp(2),
            sum(|l| l.was_cap).round_dp(2),
            sum(|l| l.now_cap).round_dp(2),
        ));
    }
    let mut kinds: BTreeMap<&str, (u64, Decimal)> = BTreeMap::new();
    for l in lines.iter().filter(|l| l.group == G_SKIP) {
        let e = kinds
            .entry(kind(l.reason.as_deref().unwrap_or_default()))
            .or_default();
        e.0 += 1;
        e.1 += l.was_missed.unwrap_or_default();
    }
    let skipped: u64 = kinds.values().map(|v| v.0).sum();
    out.push_str(&format!("{G_SKIP}: {skipped}\n"));
    for (k, (n, missed)) in kinds {
        out.push_str(&format!(
            "  {k}: {n} (упущено по старым записям {} USDT)\n",
            missed.round_dp(2)
        ));
    }
    out
}

/// Вид причины «не пересчитано» для итогов. Ошибок нет.
pub fn kind(reason: &str) -> &'static str {
    const KINDS: [(&str, &str); 10] = [
        (
            "последняя запись не старая",
            "последняя запись уже не старая (пересчитано раньше или по снимку)",
        ),
        ("DEX → биржа", "DEX → биржа (не пересчитывается)"),
        ("цикл DEX", "цикл DEX ↔ DEX"),
        ("окна у сделки нет", "у сделки нет окна"),
        ("открыто не раньше", "окно открыто после момента решения"),
        ("не писалась", "биржа сделки в окне не писалась"),
        ("файл окна", "нет или не читается файл окна"),
        (
            "полного стакана",
            "в окне нет полного стакана до момента решения",
        ),
        ("сделки нет в учёте", "сделки нет в учёте"),
        ("не задана", "не задана настройка"),
    ];
    let r = reason.to_lowercase();
    KINDS
        .iter()
        .find(|(k, _)| r.contains(&k.to_lowercase()))
        .map_or("прочее", |(_, v)| v)
}

/// Число для CSV (2 знака; нет — пусто). Ошибок нет.
fn num(v: Option<Decimal>) -> String {
    v.map(|x| x.round_dp(2).to_string()).unwrap_or_default()
}

/// Ячейка CSV: в кавычках, если есть «;», кавычка или перевод строки. Ошибок нет.
fn cell(s: &str) -> String {
    if s.contains([';', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
