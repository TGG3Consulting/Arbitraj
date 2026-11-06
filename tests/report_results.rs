//! Файл: tests/report_results.rs
//! Что проверяет: решение 193 — книга «Итоги сделок» (report/results_xlsx.rs) из тех же 400 строк, что отчёт 05.11.2026
//! (tests/data/report_2026-11-05/deals.csv), собирается; записывается в REPORT_OUT (если задан) для сверки 1 в 1 с
//! образцом tests/data/report_2026-11-05/arb_results_2026-11-05.xlsx (ячейки, формулы, стили, ширины).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::report::{results_xlsx, rows::Row}, chrono.

use arb::report::results_xlsx::workbook;
use arb::report::rows::Row;
use chrono::{TimeZone, Utc};

fn rows() -> Vec<Row> {
    let v = |c: char| match c {
        'B' => "BingX",
        'D' => "DEX",
        'M' => "MEXC",
        'K' => "KuCoin",
        _ => "Gate",
    };
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/report_2026-11-05/deals.csv"
    );
    std::fs::read_to_string(p)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            let (md, hm) = f[0].split_once(' ').unwrap();
            let (m, d) = md.split_once('-').unwrap();
            let codes: Vec<char> = f[3].chars().collect();
            Row {
                date: format!("2026-{m}-{d}"),
                time: hm.to_string(),
                hour: hm[..2].parse().unwrap(),
                coin: f[2].to_string(),
                buy: v(codes[0]).into(),
                sell: v(codes[1]).into(),
                invested: f[4].parse().unwrap(),
                fact: f[5].parse().unwrap(),
                plan: f[6].parse().unwrap(),
                duration_s: f[1].parse().unwrap(),
            }
        })
        .collect()
}

#[test]
fn builds_from_reference_rows() {
    let now = Utc.with_ymd_and_hms(2026, 11, 5, 12, 20, 0).unwrap();
    let bytes = workbook(&rows(), now).expect("книга");
    assert!(bytes.len() > 10_000);
    if let Ok(out) = std::env::var("REPORT_OUT") {
        std::fs::write(out, &bytes).unwrap();
    }
}
