//! Файл: tests/okx_book_backup.rs
//! Что делает: решение 429 — логика страховки стакана OKX (venues/okx/backup.rs): какой источник отдаётся ядру (быстрый
//! books или снимок books5), «не старее отданного» по ts, срок свежести снимка, счёт переключений.
//! Стакан — ПРИМЕР ИЗ ДОКУМЕНТАЦИИ (https://www.okx.com/docs-v5/en/, «Order Book Trading → Market Data → GET / Order
//! book», Response Example: asks [["41006.8","0.60038921","0","1"]], bids [["41006.3","0.30178218","0","2"]],
//! ts "1629966436396" — тот же пример, что tests/okx_real_data.rs book_print_docs_example; уровень в том же виде, что у
//! кадров books / books5, разбор depth.rs levels). Живых кадров books и books5 в tests/data нет (www.okx.com закрыт
//! прокси) — записать одновременно оба канала: tests/README.md, раздел OKX; docs/TASKS.md. Сдвиг ts на 1 мс ниже —
//! проверка самого сравнения «старее / не старее», а не последовательность кадров биржи.
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::okx::{backup, depth}. Моков нет.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use chrono::{TimeZone, Utc};
use serde_json::Value;

use arb::core::types::{Asset, OrderBook, VenueId};
use arb::venues::okx::backup::{Backup, Lost, Mode};
use arb::venues::okx::depth;

/// ts примера документации, мс.
const TS: i64 = 1_629_966_436_396;

/// Стакан примера документации со временем ts (мс). Ошибок нет (пример разбирается).
fn book(ts: i64) -> OrderBook {
    let d: Value = serde_json::from_str(
        r#"{"asks":[["41006.8","0.60038921","0","1"]],"bids":[["41006.3","0.30178218","0","2"]],"ts":"1629966436396"}"#,
    )
    .unwrap();
    OrderBook {
        venue: VenueId("okx".into()),
        asset: Asset {
            ticker: "BTC".into(),
            bsc_contract: None,
        },
        bids: depth::levels(&d, "bids").unwrap(),
        asks: depth::levels(&d, "asks").unwrap(),
        exchange_time: Utc.timestamp_millis_opt(ts).single(),
        received_at: Utc::now(),
    }
}

/// Значение счётчика. Ошибок нет.
fn n(c: &AtomicU64) -> u64 {
    c.load(Ordering::Relaxed)
}

#[test]
fn switch_to_snapshot_and_back() {
    let b = Backup::default();
    let alive = Arc::new(AtomicI64::new(0));
    let now = 1_000_000;
    // Старт: быстрого стакана ещё нет — снимок books5 сразу ядру (не переключение: быстрого не было).
    assert!(b.snap(book(TS), alive.clone(), now).is_some());
    assert_eq!(b.mode("BTC"), Mode::Snap);
    assert_eq!(n(&b.counts.to_snap), 0);
    // Быстрый синхронизирован: тот же ts — отдаётся (то же время построения книги), снимки дальше не отдаются.
    assert_eq!(b.fast("BTC", Some(TS)), (true, false));
    assert!(b.snap(book(TS), alive.clone(), now).is_none());
    // Разрыв prevSeqId / обрыв: снимок свежий и не старее — ядру сразу снимок.
    match b.fast_lost("BTC", Some(500), now + 100) {
        Lost::Serve(s) => assert_eq!(
            s.book.asks[0].price,
            "41006.8".parse::<rust_decimal::Decimal>().unwrap()
        ),
        _ => panic!("ожидался снимок"),
    }
    assert_eq!((n(&b.counts.lost), n(&b.counts.to_snap)), (1, 1));
    // Обрыв соединения после разрыва этой же пары — ядро уже на снимке.
    assert!(matches!(
        b.fast_lost("BTC", Some(500), now + 100),
        Lost::Keep
    ));
    // Быстрый снова синхронизирован — возврат.
    assert_eq!(b.fast("BTC", Some(TS + 1)), (true, true));
    assert_eq!(n(&b.counts.to_fast), 1);
    // Снимок старее отданного (ts меньше) — не отдаётся, пара недостоверна.
    assert!(matches!(
        b.fast_lost("BTC", Some(500), now + 100),
        Lost::Drop
    ));
    assert_eq!(n(&b.counts.no_snap), 1);
    assert_eq!(b.mode("BTC"), Mode::None);
}

#[test]
fn stale_or_unset_snapshot_not_served() {
    let b = Backup::default();
    let alive = Arc::new(AtomicI64::new(0));
    assert!(b.snap(book(TS), alive.clone(), 1_000).is_some());
    assert_eq!(b.fast("BTC", Some(TS)), (true, false));
    // Снимок пришёл 2000 мс назад, срок 1000 — устарел.
    assert!(matches!(b.fast_lost("BTC", Some(1_000), 3_000), Lost::Drop));
    // venue.book_stale_ms не задана — снимок не отдаётся.
    assert_eq!(b.fast("BTC", Some(TS)), (true, true));
    assert!(matches!(b.fast_lost("BTC", None, 1_000), Lost::Drop));
    // Пара недостоверна, страховка жива: следующий снимок books5 — сразу ядру (переключение).
    assert!(b.snap(book(TS), alive.clone(), 4_000).is_some());
    assert_eq!(n(&b.counts.to_snap), 1);
    // Страховка оборвалась, пока ядро считало по снимку, — пара недостоверна.
    assert!(b.snap_lost("BTC"));
    assert_eq!(n(&b.counts.snap_gone), 1);
    // Ядро считает по быстрому — обрыв страховки его не трогает.
    assert_eq!(b.fast("BTC", Some(TS)), (true, true));
    assert!(b.snap(book(TS), alive, 5_000).is_none());
    assert!(!b.snap_lost("BTC"));
}
