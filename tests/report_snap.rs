//! Файл: tests/report_snap.rs
//! Что проверяет: решение 257 (приёмка t90, п. 1, 2) — «Слепок по балансам»:
//! 1) настройка report.snapshot_timeout_ms не задана — слепок не снимается (причина), книга всё равно собирается, итог
//!    слепка — «нет данных», а не 0;
//! 2) страж по исходникам: слепок не идёт путём ордера и не пишет память торговли — в report/snap*.rs нет вызовов
//!    core/exec_book.rs (кэш REST-снимков и память неудач exec_book_fail.rs — общие с ордерами), trade_ops (котировка
//!    исполнения), gateway::patience::exec_scope и памяти панели «Балансы» (venue_balances / wallet_balances,
//!    balances_cache.rs); цена — только Venue::quote (котировка по текущим данным), балансы — чтением адаптера.
//!
//! 3) блок, где ни у одной строки нет цены, — «нет данных: причины», а не 0 с «неполно»; строка «под вопросом» в итог
//!    не входит.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::report, tests/report_common. Моков нет.

mod report_common;

use std::collections::HashMap;
use std::io::Read;

use arb::report::rows_time::Window;
use arb::report::snap::{Line, Place, Snapshot};
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

/// Файлы книги (zip: stored / deflate). Ошибок нет: паника — не книга.
fn unzip(b: &[u8]) -> HashMap<String, String> {
    let u16le = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]) as usize;
    let u32le = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
    let eocd = (0..b.len() - 3)
        .rev()
        .find(|&i| b[i..i + 4] == [0x50, 0x4b, 5, 6])
        .unwrap();
    let (n, mut p) = (u16le(eocd + 10), u32le(eocd + 16));
    let mut out = HashMap::new();
    for _ in 0..n {
        let (method, csize) = (u16le(p + 10), u32le(p + 20));
        let (nl, el, cl, off) = (u16le(p + 28), u16le(p + 30), u16le(p + 32), u32le(p + 42));
        let name = String::from_utf8(b[p + 46..p + 46 + nl].to_vec()).unwrap();
        let data = off + 30 + u16le(off + 26) + u16le(off + 28);
        let raw = &b[data..data + csize];
        let mut s = String::new();
        if method == 8 {
            flate2::read::DeflateDecoder::new(raw)
                .read_to_string(&mut s)
                .unwrap();
        } else {
            s = String::from_utf8(raw.to_vec()).unwrap();
        }
        out.insert(name, s);
        p += 46 + nl + el + cl;
    }
    out
}

#[test]
fn block_without_any_price_is_no_data() {
    // Тексты причин — как их пишет слепок (snap_transit.rs: перегон без записанной площадки назначения).
    let why = "площадка назначения перегона в учёте сделки не записана";
    let line = |doubt: bool| Line {
        account: "перевод".into(),
        asset: "CAKE".into(),
        qty: Ok(24.7),
        price: Err(why.into()),
        note: String::new(),
        doubt,
    };
    let snap = Snapshot {
        places: vec![Place {
            name: "В пути".into(),
            lines: vec![line(true), line(false)],
            ..Place::default()
        }],
        problem: None,
    };
    let cx = arb::report::ResultsCtx {
        snap,
        ..Default::default()
    };
    let w = Window {
        from: None,
        to: None,
        now: Utc::now(),
    };
    let x = unzip(&arb::report::results(&[], w, &cx).unwrap());
    let shared = &x["xl/sharedStrings.xml"];
    assert!(
        shared.contains(&format!("нет данных: CAKE: {why}")),
        "итог блока — «нет данных»"
    );
    assert!(
        shared.contains("не ответили площадок: 1 из 1"),
        "общий итог помечен неполным"
    );
}
