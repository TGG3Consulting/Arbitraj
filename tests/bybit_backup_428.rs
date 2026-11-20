//! Файл: tests/bybit_backup_428.rs
//! Что делает: решение 428 — выбор источника стакана Bybit (основной поток / страховка, venues/bybit/backup.rs) на
//! ЖИВЫХ кадрах orderbook.50.CAKEUSDT от 2026-11-04 (tests/data/cex_samples_2026_11_04/bybit_ws.jsonl: снимок +
//! изменения с u и seq). Стакан страховки в тестах — верх того же живого стакана с seq того же кадра: «Linear, inverse,
//! spot level 1 data has snapshot message only», seq — «You can use this field to compare different levels orderbook
//! data» (Orderbook, https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook). Кадров двух глубин одной пары
//! в записи нет — расхождение и равенство seq у разных глубин на живом потоке не проверены (docs/TASKS.md, решение 428).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bybit (backup, depth, local_book). Моков нет.

use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::time::{Duration, Instant};

use serde_json::Value;

use arb::core::types::{Asset, OrderBook, VenueId};
use arb::venues::bybit::backup::{BackupAct, Held, Hub, Lost, Main, MainAct, print};
use arb::venues::bybit::depth::{self, Msg};
use arb::venues::bybit::local_book::{Applied, LocalBook};

const T: &str = "CAKE";

/// Применённый живой кадр: снимок ли, seq, верх стакана (50 уровней) в формате ядра.
struct Fr {
    snapshot: bool,
    seq: i64,
    book: OrderBook,
}

/// Живые кадры orderbook.50.CAKEUSDT через локальный стакан (как ws_lane.rs).
fn frames() -> Vec<Fr> {
    let t = std::fs::read_to_string("tests/data/cex_samples_2026_11_04/bybit_ws.jsonl")
        .expect("запись");
    let mut lb = LocalBook::default();
    let mut out = vec![];
    for l in t.lines() {
        let Ok(v) = serde_json::from_str::<Value>(l) else {
            continue;
        };
        let Some(m) = v.get("msg").and_then(Value::as_str) else {
            continue;
        };
        if let Msg::Book {
            snapshot,
            u,
            seq,
            bids,
            asks,
            ..
        } = depth::parse(m)
            && lb.apply(snapshot, u, true, &bids, &asks) == Applied::Changed
        {
            let (b, a) = lb.top(50);
            let book = OrderBook {
                venue: VenueId("bybit".into()),
                asset: Asset {
                    ticker: T.into(),
                    chain: None,
                    contract: None,
                },
                bids: b,
                asks: a,
                exchange_time: None,
                received_at: chrono::Utc::now(),
            };
            out.push(Fr {
                snapshot,
                seq: seq.expect("seq"),
                book,
            });
        }
    }
    assert!(
        out.len() > 20 && out[0].snapshot,
        "живых кадров {}",
        out.len()
    );
    out
}

/// Отпечаток первых k уровней кадра.
fn pr(f: &Fr, k: usize) -> (usize, u64) {
    print(&f.book.bids, &f.book.asks, k)
}

/// Стакан страховки глубины 1 из живого кадра: верхний уровень того же стакана и seq кадра.
fn held1(f: &Fr) -> Held {
    let mut book = f.book.clone();
    book.bids.truncate(1);
    book.asks.truncate(1);
    Held {
        book,
        alive: Arc::new(AtomicI64::new(0)),
        seq: f.seq,
        at: Instant::now(),
    }
}

/// Страховка выключена — всё как раньше: каждый кадр основного отдаётся, потеря — стакан недостоверен.
#[test]
fn off_is_as_before() {
    let fr = frames();
    let hub = Hub::new(false, 50, 1);
    let mut gave = 0;
    for f in &fr {
        assert_eq!(
            hub.main_frame(T, f.snapshot, Some(f.seq), (0, 0), || gave += 1),
            MainAct::Give
        );
    }
    assert_eq!(gave, fr.len());
    let mut dropped = false;
    assert!(matches!(
        hub.main_lost(T, Main::Resync, Duration::from_secs(60), |h| dropped =
            h.is_none()),
        Lost::Invalidate
    ));
    assert!(dropped);
}

/// Включена: изменение до снимка не отдаётся; после снимка — все кадры живого потока (seq растёт) отдаются по порядку.
#[test]
fn main_stream_in_order() {
    let fr = frames();
    assert!(
        fr.windows(2).all(|w| w[0].seq < w[1].seq),
        "seq живого потока растёт"
    );
    let hub = Hub::new(true, 50, 1);
    hub.main_session(T, "market-1");
    assert_eq!(
        hub.main_frame(
            T,
            fr[1].snapshot,
            Some(fr[1].seq),
            pr(&fr[1], 1),
            || panic!("до снимка")
        ),
        MainAct::Skip
    );
    let mut gave = 0;
    for f in &fr {
        assert_eq!(
            hub.main_frame(T, f.snapshot, Some(f.seq), pr(f, 1), || gave += 1),
            MainAct::Give
        );
    }
    assert_eq!(gave, fr.len());
    assert!(hub.main_is_source(T));
}

/// Разрыв основного: страховка старее отданного — не отдаётся (стакан недостоверен), следующий её кадр новее
/// отданного — отдаётся сразу; изменения основного до нового снимка ядру не идут.
#[test]
fn gap_never_older() {
    let fr = frames();
    let hub = Hub::new(true, 50, 1);
    hub.main_session(T, "market-1");
    for f in &fr[..=5] {
        hub.main_frame(T, f.snapshot, Some(f.seq), pr(f, 1), || {});
    }
    assert!(matches!(
        hub.backup_frame(T, held1(&fr[3]), pr(&fr[3], 1), |_| panic!(
            "основной в порядке"
        )),
        BackupAct::Keep
    ));
    let mut dropped = false;
    let r = hub.main_lost(T, Main::Resync, Duration::from_secs(60), |h| {
        dropped = h.is_none()
    });
    assert!(
        matches!(r, Lost::Invalidate) && dropped,
        "seq страховки не больше отданного"
    );
    let mut seq = None;
    assert!(matches!(
        hub.backup_frame(T, held1(&fr[8]), pr(&fr[8], 1), |h| seq = Some(h.seq)),
        BackupAct::Give
    ));
    assert_eq!(seq, Some(fr[8].seq));
    assert!(!hub.main_is_source(T));
    assert_eq!(
        hub.main_frame(T, false, Some(fr[9].seq), pr(&fr[9], 1), || panic!(
            "ждём снимок"
        )),
        MainAct::Skip
    );
    // Потеря страховки, когда стакан ядра из неё, — стакан снимается.
    let mut inv = false;
    assert!(hub.backup_lost(T, || inv = true) && inv);
}

/// Разрыв основного при свежей страховке новее отданного — сразу её стакан; устаревшая (старше срока) — не отдаётся.
#[test]
fn gap_gives_fresh_backup() {
    let fr = frames();
    for (old, fresh) in [(false, true), (true, false)] {
        let hub = Hub::new(true, 50, 1);
        hub.main_session(T, "market-1");
        for f in &fr[..=5] {
            hub.main_frame(T, f.snapshot, Some(f.seq), pr(f, 1), || {});
        }
        let mut h = held1(&fr[7]);
        if old {
            h.at = Instant::now()
                .checked_sub(Duration::from_secs(1))
                .expect("часы");
        }
        assert!(matches!(
            hub.backup_frame(T, h, pr(&fr[7], 1), |_| {}),
            BackupAct::Keep
        ));
        let mut got = None;
        let r = hub.main_lost(T, Main::Off, Duration::from_millis(500), |h| {
            got = h.map(|h| h.seq)
        });
        if fresh {
            assert!(matches!(r, Lost::Gave) && got == Some(fr[7].seq));
            assert!(!hub.main_is_source(T));
        } else {
            assert!(
                matches!(r, Lost::Invalidate) && got.is_none(),
                "устаревшая страховка не отдаётся"
            );
        }
    }
}

/// Начало подписки: страховка успела раньше — её стакан; снимок основного с seq не новее — не отдаётся; следующий
/// кадр основного новее — возврат на основной (Back). Сверка при равном seq и том же верхнем уровне — совпало.
#[test]
fn back_to_main_and_compare() {
    let fr = frames();
    let hub = Hub::new(true, 50, 1);
    hub.main_session(T, "market-1");
    assert!(matches!(
        hub.backup_frame(T, held1(&fr[0]), pr(&fr[0], 1), |_| {}),
        BackupAct::Give
    ));
    assert_eq!(
        hub.main_frame(T, true, Some(fr[0].seq), pr(&fr[0], 1), || panic!(
            "не новее"
        )),
        MainAct::Skip
    );
    assert_eq!(hub.compares(), 1, "равный seq — сверка состоялась, совпало");
    assert_eq!(
        hub.main_frame(T, false, Some(fr[1].seq), pr(&fr[1], 1), || {}),
        MainAct::Back
    );
    assert!(matches!(
        hub.backup_frame(T, held1(&fr[1]), pr(&fr[1], 1), |_| panic!(
            "основной в порядке"
        )),
        BackupAct::Keep
    ));
    assert_eq!(hub.compares(), 2);
    assert!(hub.main_is_source(T));
}

/// Ревью 428: правило «не старее отданного» — только при смене источника. Пока стакан ядра из основного, его кадры
/// отдаются по порядку u (local_book.rs): снимок после пересподписки с меньшим seq (Orderbook: «"u"=1, which is a
/// snapshot data due to the restart of the service» — что при этом с seq, документация не пишет) не застревает. Пока
/// стакан ядра из страховки — отдаётся каждый её кадр, даже когда основной уже получил снимок, но ещё не новее отданного.
#[test]
fn same_source_never_stuck() {
    let fr = frames();
    let hub = Hub::new(true, 50, 1);
    hub.main_session(T, "market-1");
    for f in &fr[..=9] {
        hub.main_frame(T, f.snapshot, Some(f.seq), pr(f, 1), || {});
    }
    // Разрыв без страховки — стакан недостоверен; новый снимок основного (живой кадр 0 — seq меньше отданного) отдаётся.
    assert!(matches!(
        hub.main_lost(T, Main::Resync, Duration::from_secs(60), |_| {}),
        Lost::Invalidate
    ));
    let mut gave = false;
    assert_eq!(
        hub.main_frame(T, true, Some(fr[0].seq), pr(&fr[0], 1), || gave = true),
        MainAct::Give
    );
    assert!(gave && hub.main_is_source(T));
    // Стакан ядра из страховки (основной оборван), основной получил снимок не новее отданного — кадры страховки
    // по-прежнему идут ядру; основной возвращается кадром новее отданного.
    hub.main_lost(T, Main::Off, Duration::from_secs(60), |_| {});
    let mut got = vec![];
    for i in [12, 14] {
        assert!(matches!(
            hub.backup_frame(T, held1(&fr[i]), pr(&fr[i], 1), |h| got.push(h.seq)),
            BackupAct::Give
        ));
        if i == 12 {
            assert_eq!(
                hub.main_frame(T, true, Some(fr[11].seq), pr(&fr[11], 1), || panic!(
                    "не новее"
                )),
                MainAct::Skip
            );
        }
    }
    assert_eq!(got, vec![fr[12].seq, fr[14].seq]);
    assert_eq!(
        hub.main_frame(T, false, Some(fr[15].seq), pr(&fr[15], 1), || {}),
        MainAct::Back
    );
}
