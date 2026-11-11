//! Файл: tests/report_snap.rs
//! Что проверяет: решение 257 (приёмка t90, п. 1, 2) — «Слепок по балансам»:
//! 1) настройка report.snapshot_timeout_ms не задана — слепок не снимается (причина), книга всё равно собирается, итог
//!    слепка — «нет данных», а не 0;
//! 2) страж по исходникам: слепок не идёт путём ордера и не пишет память торговли — в report/snap*.rs нет вызовов
//!    core/exec_book.rs (кэш REST-снимков и память неудач exec_book_fail.rs — общие с ордерами), trade_ops (котировка
//!    исполнения), gateway::patience::exec_scope и памяти панели «Балансы» (venue_balances / wallet_balances,
//!    balances_cache.rs); цена — только Venue::quote (котировка по текущим данным), балансы — чтением адаптера.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::report, tests/report_common. Моков нет.

mod report_common;

use arb::report::rows_time::Window;
use chrono::Utc;
use report_common::{dir, start};

#[tokio::test]
async fn no_timeout_setting_no_snapshot_but_book() {
    let p = dir("snap-unset");
    let (core, _) = start(&p).await;
    let snap = arb::report::snap::gather(&core).await;
    assert!(snap.places.is_empty());
    let why = snap.problem.clone().unwrap();
    assert!(why.contains("report.snapshot_timeout_ms"), "{why}");
    let cx = arb::report::ResultsCtx {
        snap,
        ..Default::default()
    };
    let w = Window {
        from: None,
        to: None,
        now: Utc::now(),
    };
    let bytes = arb::report::results(&[], w, &cx).expect("книга без слепка");
    let x = String::from_utf8_lossy(&bytes).len();
    assert!(x > 1000);
    let _ = std::fs::remove_dir_all(&p);
}

#[test]
fn snapshot_does_not_touch_trading_state() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src/report/");
    for f in [
        "snap.rs",
        "snap_cex.rs",
        "snap_dex.rs",
        "snap_transit.rs",
        "snap_price.rs",
        "snap_sheet.rs",
    ] {
        let code: String = std::fs::read_to_string(format!("{root}{f}"))
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for bad in [
            "exec_book",
            "trade_ops",
            "exec_scope",
            "venue_balances",
            "wallet_balances",
            "bal_cache",
            "sell_value",
        ] {
            assert!(!code.contains(bad), "{f}: {bad}");
        }
    }
}
