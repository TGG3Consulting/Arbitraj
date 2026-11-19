//! Файл: tests/gate_fast_book.rs
//! Что делает: решение 421 — быстрый стакан Gate со страховкой: разбор кадров spot.obu на примерах официальной
//! документации и ведение локального стакана (fast_book.rs), выбор стакана для ядра (fast_pick.rs) на ЖИВОЙ записи
//! 04.11 (tests/data/cex_samples_2026_11_04/gate_ws.jsonl).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::gate (obu, fast_book, fast_pick, depth). Моков нет.
//!
//! Живой записи spot.obu ещё нет (docs/TASKS.md: «записать живые кадры spot.obu»). Поэтому:
//! - разбор spot.obu — на значениях примеров документации (Spot WebSocket v4, «Order Book V2 API»,
//!   https://www.gate.com/docs/developers/apiv4/ws/en/): полный снимок {t 1747054612673, full true, s «ob.BTC_USDT.50»,
//!   u 73777715168, b [["104027.1","509392"],["104027","477932"]], a [["104027.2","44617"],["104027.4","39322"]]} и
//!   изменение {t 1747054612695, s «ob.BTC_USDT.50», U 73777715169, u 73777715212, b [["104024.5","10343"],
//!   ["104014.5","509392"]], a [["104027.2","0"],["104027.4","0"]]};
//! - правило номеров (U = номер + 1, количество абсолютное, 0 — убрать) — на живых кадрах spot.order_book_update той же
//!   записи (у него те же U/u и то же правило изменений по документации), полный снимок — кадр spot.order_book
//!   (lastUpdateId = u кадра изменений при том же t — факт записи); итог сверяется с кадрами spot.order_book.

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::{Asset, OrderBook, PriceLevel};
use arb::venues::gate::depth::{self, Msg};
use arb::venues::gate::fast_book::{Applied, FastBook};
use arb::venues::gate::fast_pick::{Fast, Give};
use arb::venues::gate::obu::{self, Frame, Obu};

const REC: &str = "tests/data/cex_samples_2026_11_04/gate_ws.jsonl";

/// Кадр примера документации (значения — из раздела «Order Book V2 API»; event в примере нет).
fn doc_full() -> String {
    r#"{"time_ms":1747054612848,"channel":"spot.obu","result":{"t":1747054612673,"full":true,"s":"ob.BTC_USDT.50","u":73777715168,"b":[["104027.1","509392"],["104027","477932"]],"a":[["104027.2","44617"],["104027.4","39322"]]}}"#.to_string()
}

/// Кадр изменений примера документации.
fn doc_update() -> String {
    r#"{"time_ms":1747054612925,"channel":"spot.obu","result":{"t":1747054612695,"s":"ob.BTC_USDT.50","U":73777715169,"u":73777715212,"b":[["104024.5","10343"],["104014.5","509392"]],"a":[["104027.2","0"],["104027.4","0"]]}}"#.to_string()
}

fn d(s: &str) -> Decimal {
    s.parse().expect("число")
}

fn frame(text: &str) -> Frame {
    match obu::parse(text) {
        Some(Obu::Update(f)) => f,
        other => panic!("не кадр стакана: {other:?}"),
    }
}

#[test]
fn doc_examples_parse_and_apply() {
    assert_eq!(obu::stream("BTC_USDT", 50), "ob.BTC_USDT.50");
    assert_eq!(obu::symbol_of("ob.BTC_USDT.50"), Some("BTC_USDT"));
    assert_eq!(obu::symbol_of("BTC_USDT"), None);
    let full = frame(&doc_full());
    assert!(full.full);
    assert_eq!(
        (full.first, full.last, full.t),
        (None, 73777715168, Some(1747054612673))
    );
    let upd = frame(&doc_update());
    assert!(!upd.full);
    assert_eq!((upd.first, upd.last), (Some(73777715169), 73777715212));
    let mut lb = FastBook::default();
    // Изменение до полного снимка — ждём снимок.
    assert_eq!(lb.apply(&upd), Applied::Waiting);
    assert_eq!(lb.apply(&full), Applied::Changed);
    assert_eq!(lb.apply(&upd), Applied::Changed);
    assert_eq!(lb.version(), Some(73777715212));
    let (bids, asks) = lb.top(50);
    let px: Vec<Decimal> = bids.iter().map(|l| l.price).collect();
    assert_eq!(
        px,
        vec![d("104027.1"), d("104027"), d("104024.5"), d("104014.5")]
    );
    // Количество «0» убрало оба уровня asks.
    assert!(asks.is_empty());
    // Тот же кадр ещё раз: U ≠ номер + 1 — разрыв, стакан сброшен.
    assert!(matches!(lb.apply(&upd), Applied::Gap(_)));
    assert!(!lb.synced());
}

/// Кадры стаканов записи (spot.order_book и spot.order_book_update, по порядку): (канал, result).
fn recorded() -> Vec<(String, Value)> {
    let text = std::fs::read_to_string(REC).expect("запись");
    let mut out = vec![];
    for line in text.lines() {
        let o: Value = serde_json::from_str(line).expect("JSON строки");
        let Some(m) = o.get("msg").and_then(Value::as_str) else {
            continue;
        };
        let m: Value = serde_json::from_str(m).expect("JSON кадра");
        if m.get("event").and_then(Value::as_str) != Some("update") {
            continue;
        }
        let ch = m["channel"].as_str().unwrap_or_default().to_string();
        if ch == "spot.order_book" || ch == "spot.order_book_update" {
            out.push((ch, m["result"].clone()));
        }
    }
    out
}

/// Кадр spot.order_book_update → кадр формата spot.obu (те же поля U, u, t, b, a).
fn from_update(r: &Value) -> Frame {
    let mut x = r.clone();
    x["s"] = Value::from("ob.CAKE_USDT.50");
    frame(&serde_json::json!({ "channel": "spot.obu", "result": x }).to_string())
}

/// Кадр spot.order_book → полный снимок формата spot.obu (u = lastUpdateId).
fn full_from_book(r: &Value) -> Frame {
    let x = serde_json::json!({ "t": r["t"], "full": true, "s": "ob.CAKE_USDT.50", "u": r["lastUpdateId"],
        "b": r["bids"], "a": r["asks"] });
    frame(&serde_json::json!({ "channel": "spot.obu", "result": x }).to_string())
}

/// Снимок spot.order_book записи → (lastUpdateId, OrderBook).
fn snapshot(r: &Value, asset: &Asset) -> (i64, OrderBook) {
    let text = serde_json::json!({ "channel": "spot.order_book", "event": "update", "result": r })
        .to_string();
    match depth::parse(&text) {
        Msg::Depth {
            update_id,
            bids,
            asks,
            exchange_time,
            ..
        } => (
            update_id,
            depth::to_book(asset.clone(), bids, asks, exchange_time),
        ),
        _ => panic!("снимок не разобран"),
    }
}

fn top5(b: &OrderBook) -> (Vec<PriceLevel>, Vec<PriceLevel>) {
    (
        b.bids.iter().take(5).cloned().collect(),
        b.asks.iter().take(5).cloned().collect(),
    )
}

#[test]
fn recorded_updates_rebuild_snapshots() {
    let rec = recorded();
    let books: Vec<&Value> = rec
        .iter()
        .filter(|(c, _)| c == "spot.order_book")
        .map(|(_, r)| r)
        .collect();
    let mut lb = FastBook::default();
    assert_eq!(lb.apply(&full_from_book(books[0])), Applied::Changed);
    let (mut same, mut checked) = (0, 0);
    for (ch, r) in &rec {
        if ch != "spot.order_book_update" || r["u"].as_i64() <= lb.version() {
            continue;
        }
        assert_eq!(
            lb.apply(&from_update(r)),
            Applied::Changed,
            "номера живой записи идут подряд"
        );
        let Some(b) = books
            .iter()
            .find(|b| b["lastUpdateId"].as_i64() == lb.version())
        else {
            continue;
        };
        let asset = Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        };
        let (_, snap) = snapshot(b, &asset);
        let (bids, asks) = lb.top(5);
        checked += 1;
        same += usize::from((bids, asks) == top5(&snap));
    }
    assert!(checked > 40, "сверено кадров: {checked}");
    assert_eq!(
        same, checked,
        "верх локального стакана = снимок с тем же номером"
    );
}

#[test]
fn pick_fast_then_snapshot_on_gap() {
    let rec = recorded();
    let asset = Asset {
        ticker: "CAKE".into(),
        bsc_contract: None,
    };
    let sym = "CAKE_USDT";
    let mut fast = Fast::new(Some(50), 20);
    // Порядок записи: 0 OB(t086) 1 UP(=062) 2 OB(t169) 3 UP(t169) 4 UP(t314) 5 OB(t314) 6 UP(t393) 7 OB(t393)
    // 8 OB(t526) 9 UP(t526) 10 OB(t612) 11 UP(t612) 12 UP(t902) 13 OB(t902) 14 OB(t949).
    let (_, ob0) = snapshot(&rec[0].1, &asset);
    assert!(
        fast.snapshot(sym, ob0).is_some(),
        "быстрого ещё нет — снимок ядру"
    );
    // Полный снимок потока с тем же t, что отданный снимок, — не новее: ядру не отдаётся.
    assert!(matches!(
        fast.frame(sym, &asset, &full_from_book(&rec[0].1)),
        (Give::Nothing, None)
    ));
    assert!(
        fast.snapshot(sym, snapshot(&rec[2].1, &asset).1).is_some(),
        "ядро ещё на снимках"
    );
    assert!(matches!(
        fast.frame(sym, &asset, &from_update(&rec[3].1)),
        (Give::Nothing, None)
    ));
    let (Give::Book(b4), None) = fast.frame(sym, &asset, &from_update(&rec[4].1)) else {
        panic!("быстрый новее")
    };
    assert_eq!(
        top5(&b4),
        top5(&snapshot(&rec[5].1, &asset).1),
        "быстрый = снимок с тем же номером"
    );
    assert!(fast.on_fast(sym));
    assert!(
        fast.snapshot(sym, snapshot(&rec[5].1, &asset).1).is_none(),
        "на быстром снимок придержан"
    );
    assert!(matches!(
        fast.frame(sym, &asset, &from_update(&rec[6].1)),
        (Give::Book(_), None)
    ));
    for i in [7, 8, 10] {
        assert!(fast.snapshot(sym, snapshot(&rec[i].1, &asset).1).is_none());
    }
    // Кадр 9 потерян: кадр 11 — разрыв; сразу придержанный снимок 10 (t612 новее отданного t393).
    let (g, gap) = fast.frame(sym, &asset, &from_update(&rec[11].1));
    assert!(gap.is_some());
    let Give::Book(fb) = g else {
        panic!("ожидался снимок")
    };
    assert_eq!(
        fb.exchange_time.map(|t| t.timestamp_millis()),
        rec[10].1["t"].as_i64()
    );
    assert_eq!(
        fast.next_resync().as_deref(),
        Some(sym),
        "пара в очереди пересинхронизации"
    );
    assert!(!fast.on_fast(sym));
    assert!(matches!(
        fast.frame(sym, &asset, &from_update(&rec[12].1)),
        (Give::Nothing, None)
    ));
    assert!(
        fast.snapshot(sym, snapshot(&rec[13].1, &asset).1).is_some(),
        "до синхронизации — снимки"
    );
    // Полный снимок после переподписки (t949 новее t902) — снова быстрый.
    assert!(matches!(
        fast.frame(sym, &asset, &full_from_book(&rec[14].1)),
        (Give::Book(_), None)
    ));
    assert!(fast.on_fast(sym));
}

#[test]
fn without_level_snapshots_pass_through() {
    let rec = recorded();
    let asset = Asset {
        ticker: "CAKE".into(),
        bsc_contract: None,
    };
    let mut fast = Fast::new(None, 20);
    assert!(fast.level().is_none());
    for i in [0, 2, 5] {
        assert!(
            fast.snapshot("CAKE_USDT", snapshot(&rec[i].1, &asset).1)
                .is_some()
        );
    }
    fast.resync("CAKE_USDT");
    assert!(!fast.pending());
}
