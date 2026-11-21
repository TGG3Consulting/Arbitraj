//! Файл: tests/report_old_format.rs
//! Что проверяет: решение 257 (приёмка t90, п. 5) — конец сделки по хронологии и в старых форматах учёта: 400 записанных
//! сделок 01–05.11 (tests/data/report_2026-11-05/deals.csv), закрытых до 86832aa (строка «Closed: …» — живой текст,
//! docs/notes/usdt-return-close-2026-11-04.md), и остановленная при перезапуске в тексте до 86832aa: конец находится,
//! длительность = «Конец − Старт» до секунды, начисления «восстановлено» — по дням закрытия (несколько дней).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::report, tests/report_common. Моков нет.

mod report_common;

use std::collections::{BTreeMap, BTreeSet};

use arb::report::rows_time::Window;
use chrono::{Duration, Utc};
use report_common::{old_format, old_restart, rows};

#[test]
fn old_close_texts_and_days() {
    let mut deals = old_format();
    deals.push(old_restart(8, 1234));
    let w = Window {
        from: None,
        to: None,
        now: Utc::now(),
    };
    let dex: BTreeSet<String> = ["bsc_dex".to_string()].into();
    let got = arb::report::rows::rows(&deals, &w, &dex);
    assert_eq!(got.len(), deals.len());
    let rec = rows();
    for (k, (st, dur, coin, ..)) in rec.iter().enumerate() {
        let r = got
            .iter()
            .find(|x| x.id == format!("o{k:03}{coin}"))
            .unwrap();
        assert_eq!(r.end, Some(*st + Duration::seconds(*dur)), "{}", r.id);
        assert_eq!(r.duration_s, Some(*dur));
        assert!(!r.status.contains("не найден"));
    }
    let oh = got.iter().find(|x| x.id.starts_with("oh")).unwrap();
    assert_eq!(
        oh.duration_s,
        Some(1234),
        "старый текст «прервана перезапуском»"
    );
    let led = arb::report::day_rows::day_rows(&deals, &w, &dex, &BTreeMap::new());
    let days: BTreeSet<_> = led.iter().map(|x| x.date).collect();
    assert_eq!(days.len(), 5, "01–05.11: {days:?}");
    assert!(led.iter().all(|x| !x.booked));
}
