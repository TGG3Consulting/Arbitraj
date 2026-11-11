//! Файл: tests/report_signals.rs
//! Что проверяет: решение 255 — отчёт «Сигналы и сделки».
//! 1) Периоды: свой интервал строго «дд.мм.гг – дд.мм.гг» (тире «–», «—», «-»), ошибки — понятным текстом; границы
//!    текущего дня, N дней и интервала (UTC, обе даты включительно).
//! 2) Книга на записанных данных: файл окна из записанных кадров стакана BingX CAKE-USDT@depth20
//!    (tests/data/bingx/ws_spot_2026-10-23.jsonl) и сделки канала @trade из официальной документации BingX (живой записи
//!    ленты в репозитории нет — tests/bingx_window_tap.rs); точка цены DEX — настоящая котировка по записи пулов CAKE
//!    routes_2026-10-29 (блок 130521896) с состояниями пулов маршрута (window.dex_pool_state); эпизод направления — с
//!    состоянием пулов той же записи. Колонки главного листа — ровно как в задаче Тиграна; сделка привязана по списку
//!    окна; сделка без окна — своя строка с причиной; сигнал без сделки — «трейда не было»; DEX — «ДЕКС», BingX — «ЦЕКС».
//! 3) Деление файла: предел меньше книги — части по сигналам (каждая не больше предела, строки не теряются); один
//!    большой сигнал — доли объёмных строк; не делится — ошибка с причиной, без молчаливой обрезки.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::report::sig_*, arb::core::{signals_src, window_tap, window_dex}, tests/route_common.

mod route_common;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use alloy_primitives::U256;
use arb::core::deal::Deal;
use arb::core::episode::EpisodeInfo;
use arb::core::signals_src::SignalsSrc;
use arb::core::types::{
    Asset, Direction, Quote, Side, TokenTaxes, TradeRequest, TradeResult, TradeStatus, VenueId,
};
use arb::core::window::{WinDeal, WinVenue, WindowInfo};
use arb::core::window_dex::DexPoint;
use arb::core::window_tap::line;
use arb::gateway::contract::VenueKind;
use arb::gateway::tap::{TapData, TapFrame, TapInfo, TapStream};
use arb::report::sig_build::build;
use arb::report::sig_model::Sig;
use arb::report::sig_parts::files;
use arb::report::sig_period::{Period, parse_range};
use arb::report::sig_stream::stream;
use arb::report::sig_xlsx::MAIN;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{Quoting, direct_route};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use route_common::{CAKE, USDT};

/// Сделка канала @trade из официальной документации BingX docs-v3 (dataUpdateDemo) — как tests/bingx_window_tap.rs.
const DOCS_TRADE: &str = r#"{"code":0,"data":{"E":1760066129336,"T":1760066129316,"e":"trade","m":true,"p":"121123.47","q":"0.003189","s":"BTC-USDT","t":"172356086"},"dataType":"BTC-USDT@trade","success":true,"timestamp":1760066129336}"#;

fn day(d: u32, m: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, m, d).unwrap()
}

#[test]
fn custom_range_strict_ddmmyy() {
    for t in [
        "01.11.26 – 07.11.26",
        "01.11.26-07.11.26",
        " 01.11.26 — 07.11.26 ",
    ] {
        assert_eq!(
            parse_range(t),
            Ok(Period::Range(day(1, 11), day(7, 11))),
            "{t}"
        );
    }
    for t in [
        "1.11.26 – 7.11.26",
        "01.11.2026 – 07.11.2026",
        "01.11.26",
        "31.02.26 – 01.03.26",
        "07.11.26 – 01.11.26",
        "aa.bb.cc – 01.11.26",
    ] {
        let e = parse_range(t).unwrap_err();
        assert!(e.contains("дд.мм.гг"), "{t}: {e}");
    }
    let now = Utc.with_ymd_and_hms(2026, 11, 11, 15, 30, 0).unwrap();
    let r = parse_range("01.11.26 – 07.11.26").unwrap().bounds(now);
    assert_eq!(
        r.0,
        Some(Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap())
    );
    assert_eq!(
        r.1,
        Some(Utc.with_ymd_and_hms(2026, 11, 8, 0, 0, 0).unwrap()),
        "вторая дата — включительно"
    );
    assert_eq!(
        Period::Today.bounds(now),
        (
            Some(Utc.with_ymd_and_hms(2026, 11, 11, 0, 0, 0).unwrap()),
            None
        )
    );
    assert_eq!(
        Period::Days(3).bounds(now),
        (Some(now - Duration::days(3)), None)
    );
    assert_eq!(Period::All.bounds(now), (None, None));
}

/// Котировка продажи CAKE на DEX по записи пулов (прямой путь). Ошибок нет (тест падает).
async fn dex_quote(q: &Quoting, qty: u32) -> Quote {
    let t = TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    };
    let v = Via {
        q,
        coin: CAKE,
        usdt: USDT,
        side: Side::Sell,
        taxes: &t,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (CAKE, "CAKE"),
        mids: &q.mids,
    };
    let entries: Vec<Entry> = q
        .reg
        .by_pair(CAKE, USDT)
        .into_iter()
        .filter(|e| e.ticker == "CAKE")
        .collect();
    let f = |_: &Entry| true;
    direct_route::quote(
        &v,
        &names,
        &cake(),
        Decimal::from(qty),
        entries,
        &[],
        &f,
        U256::from(1_000_000_000u64),
    )
    .await
    .expect("котировка по записи")
}

fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    }
}

fn dir() -> Direction {
    Direction {
        asset: cake(),
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId("bsc_dex".into()),
    }
}

/// Записанные кадры стакана BingX (@depth) и время BingX каждого, мс.
fn frames() -> Vec<(String, i64)> {
    std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl")
        .unwrap()
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["m"]
                .as_str()
                .unwrap()
                .to_string()
        })
        // Только кадры со стаканом (поле data); ответ об ошибке подписки «NOSUCH-USDT@depth5» — не стакан.
        .filter(|m| m.contains("@depth") && m.contains("\"data\""))
        .map(|m| {
            let ts = serde_json::from_str::<Value>(&m).unwrap()["timestamp"]
                .as_i64()
                .unwrap();
            (m, ts)
        })
        .collect()
}

/// Пишет файл zstd строками. Ошибок нет (тест падает).
fn zst(path: &PathBuf, rows: &[String]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut e = zstd::Encoder::new(std::fs::File::create(path).unwrap(), 3).unwrap();
    for r in rows {
        writeln!(e, "{r}").unwrap();
    }
    e.finish().unwrap();
}

fn trade(venue: &str, side: Side, qty: Decimal, price: Decimal, at: DateTime<Utc>) -> TradeResult {
    TradeResult {
        request: TradeRequest {
            venue: VenueId(venue.into()),
            asset: cake(),
            side,
            qty,
            worst_price: price,
            deal_id: "d1".into(),
            route: None,
            exit_position: false,
            remeasure: None,
        },
        status: TradeStatus::Filled,
        filled_qty: qty,
        avg_price: price,
        fees: vec![],
        venue_ref: String::new(),
        chain: None,
        finished_at: at,
        executor: None,
    }
}

/// Окружение: папка с файлом окна и эпизода, данные отчёта, время начала окна.
async fn scene(tag: &str) -> (SignalsSrc, PathBuf) {
    let root = std::env::temp_dir().join(format!("arb_sig_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let p = route_common::parts().await;
    let q = dex_quote(&p.quoting(), 10).await;
    let states: Vec<_> = p.registry.all().into_iter().map(|e| e.pool).collect();
    let fr = frames();
    let start = DateTime::from_timestamp_millis(fr[0].1).unwrap();
    let mut rows = vec![json!({"type": "open"}).to_string()];
    let frame = |s: TapStream, text: &str, ms: i64| {
        let f = TapFrame {
            venue: VenueId("bingx".into()),
            stream: s,
            conn: "c".into(),
            recv_ns: ms * 1_000_000,
            data: TapData::Text(text.into()),
        };
        line(&f).to_string()
    };
    for (m, ts) in &fr {
        rows.push(frame(TapStream::Book, m, *ts));
    }
    rows.push(frame(TapStream::Trades, DOCS_TRADE, fr[0].1 + 1));
    let point = DexPoint {
        at: q.at,
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId("bsc_dex".into()),
        venue: VenueId("bsc_dex".into()),
        side: Side::Sell,
        price: q.avg_price,
        qty: q.fillable_qty,
        block: q.route.as_ref().map(|r| r.block),
        pools: vec![],
        deal: None,
        states: states
            .iter()
            .map(|p| serde_json::to_value(p).unwrap())
            .collect(),
    };
    rows.push(json!({"type": "dex", "point": point}).to_string());
    zst(&root.join("windows/w1.jsonl.zst"), &rows);
    let calc = json!({"type": "calc", "t": start, "pools": states, "books": [], "card": null});
    zst(&root.join("episodes/e1.jsonl.zst"), &[calc.to_string()]);
    let end = start + Duration::seconds(30);
    let info = TapInfo {
        book_channel: "CAKE-USDT@depth20".into(),
        trades_channel: Ok("CAKE-USDT@trade".into()),
        trades_form: "по одной".into(),
        diff_channel: Err("не задано".into()),
    };
    let w = WindowInfo {
        id: "w1".into(),
        asset: cake(),
        dry: false,
        directions: vec![dir()],
        venues: vec![WinVenue {
            venue: VenueId("bingx".into()),
            asset: cake(),
            since: start,
            tap: Some(Ok(info)),
        }],
        started_at: start,
        ended_at: Some(end),
        max_profit: Decimal::ONE,
        max_profit_unbounded: None,
        deals: vec![WinDeal {
            id: "d1".into(),
            open: false,
            step: "Closed".into(),
            result_usd: None,
        }],
        outcome: None,
        frames: 0,
        lost: 0,
        bytes: 0,
        dex_points: 1,
        maxwins: vec![],
        stopped: None,
        close_reason: None,
    };
    let mut d1 = Deal::new("d1".into(), dir(), json!({}), Decimal::ONE);
    d1.started_at = start + Duration::seconds(1);
    d1.buy = Some(trade(
        "bingx",
        Side::Buy,
        q.fillable_qty,
        q.avg_price,
        start + Duration::seconds(2),
    ));
    d1.sells = vec![trade(
        "bsc_dex",
        Side::Sell,
        q.fillable_qty,
        q.avg_price,
        start + Duration::seconds(9),
    )];
    // Сделка без окна: через час после окна.
    let mut d2 = Deal::new("d2".into(), dir(), json!({}), Decimal::ONE);
    d2.started_at = start + Duration::hours(1);
    let ep = EpisodeInfo {
        id: "e1".into(),
        direction: dir(),
        dry: false,
        started_at: start,
        ended_at: Some(end),
        max_profit: Decimal::ONE,
        max_profit_unbounded: None,
        records: 1,
        close_reason: None,
        stopped: None,
    };
    let decode: arb::core::signals_src::Decode = Arc::new(|v, s, d| {
        if v.0 != "bingx" {
            return Err("не BingX".into());
        }
        arb::venues::bingx::tap::decode(s, d).map_err(|e| e.message)
    });
    let src = SignalsSrc {
        windows: vec![w],
        episodes: vec![ep],
        deals: vec![d1, d2],
        kinds: HashMap::from([
            ("bingx".to_string(), VenueKind::Cex),
            ("bsc_dex".to_string(), VenueKind::Dex),
        ]),
        decode,
        window_dir: root.join("windows"),
        episode_dir: root.join("episodes"),
        windows_off: None,
        episodes_off: None,
        pool_state_off: None,
        now: end + Duration::hours(2),
    };
    (src, root)
}

/// Файлы xlsx (zip) по имени — как tests/report_missed.rs. Ошибок нет (паника — не zip).
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

/// Строк данных на листе (без шапки). Ошибок нет.
fn data_rows(sheet: &str) -> usize {
    sheet.matches("<row ").count().saturating_sub(1)
}

#[tokio::test]
async fn workbook_from_recorded_window() {
    let (src, root) = scene("main").await;
    let sigs = build(&src, None, None);
    assert_eq!(sigs.len(), 2, "окно и сделка без окна");
    let w = &sigs[0];
    assert_eq!(w.key, "w1");
    let r = &w.rows[0];
    assert_eq!((r.venue1.as_str(), r.venue2.as_str()), ("BingX", "DEX"));
    assert_eq!((r.kind1.as_str(), r.kind2.as_str()), ("ЦЕКС", "ДЕКС"));
    assert!(
        r.trade1_at.contains("сделка d1") && r.trade2_at.contains("сделка d1"),
        "{r:?}"
    );
    assert!(
        r.trade1_size.contains("CAKE /") && r.trade1_size.contains("USDT"),
        "{}",
        r.trade1_size
    );
    assert_eq!(w.ticks.len(), 1, "сделка документации BingX");
    assert_eq!(
        w.books.len(),
        frames().len(),
        "каждый записанный кадр стакана"
    );
    assert!(
        w.books
            .iter()
            .all(|b| b.bid.is_some() && b.ask.is_some() && b.bid < b.ask)
    );
    let states = w
        .liq
        .iter()
        .filter(|x| x.source.contains("состояние пула"))
        .count();
    assert!(states >= 2 && w.liq.iter().any(|x| x.source == "окно: точка цены DEX"));
    assert!(
        w.liq
            .iter()
            .filter(|x| !x.state.is_empty())
            .all(|x| !x.state.contains("\"0x")),
        "числа — десятичные"
    );
    assert!(
        r.ticks.starts_with("ключ w1") && r.liquidity.contains("точек цены DEX 1"),
        "{} | {}",
        r.ticks,
        r.liquidity
    );
    let o = &sigs[1].rows[0];
    assert!(
        o.win_start.starts_with("нет данных") && o.trade1_at.contains("покупки нет"),
        "{o:?}"
    );
    // Период: только сделка без окна (час после окна).
    let late = build(&src, Some(w.at + Duration::minutes(30)), None);
    assert_eq!(late.len(), 1);
    let f = files(
        &sigs,
        usize::MAX,
        "arb_signals_t",
        "Отчёт",
        &["пояснение".into()],
    )
    .unwrap();
    assert_eq!(f.len(), 1);
    let z = unzip(&f[0].bytes);
    let shared = &z["xl/sharedStrings.xml"];
    for c in MAIN {
        assert!(shared.contains(&format!(">{c}<")), "колонка «{c}»");
    }
    assert_eq!(MAIN.len(), 15);
    for s in [
        "Сигналы",
        "Тики",
        "Стаканы",
        "Ликвидность пула",
        "Пояснения",
    ] {
        assert!(
            z["xl/workbook.xml"].contains(&format!("name=\"{s}\"")),
            "лист {s}"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn big_file_splits_without_loss() {
    let (src, root) = scene("split").await;
    let sigs = build(&src, None, None);
    let whole = files(&sigs, usize::MAX, "p", "c", &[]).unwrap();
    let max = whole[0].bytes.len() - 1;
    let parts = files(&sigs, max, "p", "c", &[]).unwrap();
    assert!(parts.len() >= 2);
    assert!(parts.iter().all(|p| p.bytes.len() <= max));
    assert!(
        parts[0].name.ends_with("_part1of2.xlsx") || parts.len() > 2,
        "{}",
        parts[0].name
    );
    let main: usize = parts
        .iter()
        .map(|p| data_rows(&unzip(&p.bytes)["xl/worksheets/sheet1.xml"]))
        .sum();
    assert_eq!(
        main,
        sigs.iter().map(|s| s.rows.len()).sum::<usize>(),
        "строки сигналов не теряются"
    );
    // Один большой сигнал — доли его объёмных строк; строки «Стаканов» не теряются.
    let one: Vec<Sig> = vec![sigs[0].clone()];
    let full = files(&one, usize::MAX, "p", "c", &[]).unwrap()[0]
        .bytes
        .len();
    let cut = files(&one, full - 1, "p", "c", &[]).unwrap();
    assert!(
        cut.len() >= 2 && cut[0].caption.contains("доля"),
        "{}",
        cut[0].caption
    );
    let books: usize = cut
        .iter()
        .map(|p| data_rows(&unzip(&p.bytes)["xl/worksheets/sheet3.xml"]))
        .sum();
    assert_eq!(books, one[0].books.len());
    // Не делится до предела — ошибка, не обрезка.
    let e = files(&one, 1000, "p", "c", &[]).unwrap_err();
    assert!(e.contains("не делится"), "{e}");
    // Поток (как отправка в Telegram): один файл — без номера части; предел меньше — части _part1, _part2 …, строки
    // сигналов не теряются; приёмник отказался — ошибка, а не тихий конец.
    let mut got = vec![];
    let n = stream(&src, None, None, usize::MAX, "s", "c", &[], &mut |f| {
        got.push(f);
        true
    })
    .unwrap();
    assert_eq!((n, got[0].name.as_str()), (1, "s.xlsx"));
    let mut got = vec![];
    let n = stream(&src, None, None, max, "s", "c", &[], &mut |f| {
        got.push(f);
        true
    })
    .unwrap();
    assert!(n >= 2 && got[0].name == "s_part1.xlsx" && got.iter().all(|f| f.bytes.len() <= max));
    let main: usize = got
        .iter()
        .map(|p| data_rows(&unzip(&p.bytes)["xl/worksheets/sheet1.xml"]))
        .sum();
    assert_eq!(main, sigs.iter().map(|s| s.rows.len()).sum::<usize>());
    assert!(stream(&src, None, None, max, "s", "c", &[], &mut |_| false).is_err());
    let _ = std::fs::remove_dir_all(root);
}

/// Замер (не проверка): сколько байт книги на строку объёмных листов — записанные кадры BingX и сделка документации,
/// повторённые REPEAT раз в одном окне. Запуск: с флагами --ignored --nocapture.
#[tokio::test]
#[ignore = "замер размера файла на объёме, не проверка"]
async fn measure_bytes_per_row() {
    const REPEAT: i64 = 50_000;
    let (mut src, root) = scene("measure").await;
    let fr = frames();
    let tap = |s: TapStream, text: &str, ms: i64| {
        let f = TapFrame {
            venue: VenueId("bingx".into()),
            stream: s,
            conn: "c".into(),
            recv_ns: ms * 1_000_000,
            data: TapData::Text(text.into()),
        };
        line(&f).to_string()
    };
    let mut rows = vec![];
    for i in 0..REPEAT {
        for (m, ts) in &fr {
            rows.push(tap(TapStream::Book, m, ts + i));
        }
        rows.push(tap(TapStream::Trades, DOCS_TRADE, fr[0].1 + i));
    }
    zst(&root.join("windows/w1.jsonl.zst"), &rows);
    let on_disk = std::fs::metadata(root.join("windows/w1.jsonl.zst"))
        .unwrap()
        .len();
    src.deals.truncate(1);
    let t0 = std::time::Instant::now();
    let sigs = build(&src, None, None);
    let n = sigs[0].ticks.len() + sigs[0].books.len() + sigs[0].liq.len();
    let f = files(&sigs, usize::MAX, "m", "c", &[]).unwrap();
    eprintln!(
        "строк {n} (тики {}, стаканы {}), файл окна {on_disk} байт, xlsx {} байт, {:.1} байт/строка, {:?}",
        sigs[0].ticks.len(),
        sigs[0].books.len(),
        f[0].bytes.len(),
        f[0].bytes.len() as f64 / n as f64,
        t0.elapsed()
    );
    let _ = std::fs::remove_dir_all(root);
}
