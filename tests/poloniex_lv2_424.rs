//! Файл: tests/poloniex_lv2_424.rs
//! Что делает: решение 424 — быстрый стакан Poloniex (канал book_lv2) со страховкой снимками канала book, чистая логика
//! адаптера на примерах ОФИЦИАЛЬНОЙ документации (живых записей book_lv2 нет — docs/FIRST_RUN.md, docs/TASKS.md):
//! - разбор кадров book_lv2 (lv2.rs): пример «snapshot» и пример «update» офдоки Market Data → Book Level 2
//!   (https://api-docs.poloniex.com/spot/websocket/market-data; в офдоке уровни снимка обрезаны «...» — взяты три
//!   показанных уровня каждой стороны, остальное — как в примере);
//! - локальный стакан (local_book.rs): снимок, изменение по цепочке lastId = id прежней записи, разрыв при повторе того
//!   же изменения («If id of the last message does not match lastId of the current message»), ожидание снимка;
//! - выбор стакана (fast_book.rs): до синхронизации ядру — снимок book (пример офдоки Market Data → Book), после
//!   снимка book_lv2 — быстрый, разрыв — страховка, но не старше уже отданного (createTime снимка book в примере офдоки
//!   раньше createTime book_lv2 — ядру не уходит, стакан пары недостоверен); ревью 424: два несовпавших снимка book
//!   подряд при неизменном быстром, но созданные раньше него (пример офдоки), — не разрыв;
//! - `#[ignore]` до записи с сервера бота: кадры book_lv2 по всей цепочке без разрывов.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::poloniex::{depth, lv2, local_book, fast_book}.

use std::time::Duration;

use rust_decimal::Decimal;

use arb::venues::poloniex::depth::{self, Msg};
use arb::venues::poloniex::fast_book::{Act, FastBooks};
use arb::venues::poloniex::local_book::{Applied, LocalBook};
use arb::venues::poloniex::lv2::Lv2;

/// Пример «snapshot» канала book_lv2 (офдока Poloniex, Market Data → Book Level 2), три показанных уровня сторон.
const LV2_SNAPSHOT_DOC: &str = r#"{"channel":"book_lv2","action":"snapshot","data":[{"symbol":"BTC_USDT","asks":[["6.16","0.6"],["6.17","1"],["6.18","1"]],"bids":[["5.65","0.02"],["5.61","1.68"],["5.6","25.38"]],"createTime":1653029116343,"lastId":10409,"id":10410,"ts":1652774727337}]}"#;

/// Пример «update» канала book_lv2 (офдока Poloniex, Market Data → Book Level 2).
const LV2_UPDATE_DOC: &str = r#"{"channel":"book_lv2","action":"update","data":[{"symbol":"BTC_USDT","asks":[["6.35","3"]],"bids":[["5.65","0.02"]],"createTime":1653029116345,"lastId":10410,"id":10421,"ts":1653029116529}]}"#;

/// Пример ответа канала book (офдока Poloniex, Market Data → Book; тот же, что в tests/ioc_263.rs).
const BOOK_DOC: &str = r#"{"channel":"book","data":[{"symbol":"BTC_USDT","createTime":1648052239156,"asks":[],"bids":[["40001.5","2.87"],["39999.4","1"]],"id":123456,"ts":1648052239192},{"symbol":"BTC_USDT","createTime":1648052239156,"asks":[],"bids":[["40001","2.87"],["39999","1"]],"id":345678,"ts":1648052239192}]}"#;

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Одна запись book_lv2 из кадра.
fn lv2(frame: &str) -> Lv2 {
    let Msg::Lv2(mut list) = depth::parse(frame) else {
        panic!("кадр book_lv2 офдоки не разобран");
    };
    assert_eq!(list.len(), 1);
    list.remove(0)
}

#[test]
fn lv2_doc_frames_parse() {
    let s = lv2(LV2_SNAPSHOT_DOC);
    assert!(s.snapshot);
    assert_eq!(
        (s.symbol.as_str(), s.id, s.last_id),
        ("BTC_USDT", Some(10410), Some(10409))
    );
    assert_eq!((s.asks.len(), s.bids.len()), (3, 3));
    assert_eq!(
        s.exchange_time.map(|t| t.timestamp_millis()),
        Some(1653029116343)
    );
    let u = lv2(LV2_UPDATE_DOC);
    assert!(!u.snapshot);
    assert_eq!((u.id, u.last_id), (Some(10421), Some(10410)));
    assert_eq!((u.asks[0].price, u.asks[0].qty), (d("6.35"), d("3")));
}

#[test]
fn local_book_chain_and_gap() {
    let (snap, upd) = (lv2(LV2_SNAPSHOT_DOC), lv2(LV2_UPDATE_DOC));
    let mut lb = LocalBook::default();
    assert_eq!(
        lb.apply(&upd),
        Applied::Waiting,
        "изменение до снимка — ждём снимок"
    );
    assert!(!lb.synced());
    assert_eq!(lb.apply(&snap), Applied::Changed);
    // lastId 10410 = id снимка — цепочка цела (id вырос не на 1: 10410 → 10421, как в примере офдоки).
    assert_eq!(lb.apply(&upd), Applied::Changed);
    let (bids, asks) = lb.top(20);
    assert_eq!(bids[0].price, d("5.65"));
    assert_eq!(bids[0].qty, d("0.02"));
    assert_eq!(asks.len(), 4, "6.35 добавлен к трём уровням снимка");
    assert_eq!(asks[3].price, d("6.35"));
    assert_eq!(lb.top(2).1.len(), 2, "ядру — не больше venue.depth_levels");
    // То же изменение снова: lastId 10410 ≠ id прежней записи 10421 — разрыв, стакан сброшен.
    assert!(matches!(lb.apply(&upd), Applied::Gap(_)));
    assert!(!lb.synced());
    assert_eq!(
        lb.apply(&upd),
        Applied::Waiting,
        "после разрыва — до нового снимка"
    );
    assert_eq!(
        lb.apply(&snap),
        Applied::Changed,
        "новый снимок — снова синхронизирован"
    );
}

#[test]
fn fast_book_insurance_never_older() {
    let (snap, upd) = (lv2(LV2_SNAPSHOT_DOC), lv2(LV2_UPDATE_DOC));
    let Msg::Books(mut books) = depth::parse(BOOK_DOC) else {
        panic!("кадр book офдоки не разобран");
    };
    let stale = || Some(Duration::from_secs(3600));
    let mut fb = FastBooks::default();
    // Быстрый не синхронизирован — ядру снимок book, как до решения 424.
    assert!(matches!(
        fb.on_book(books[0].clone(), 20, stale),
        (Act::Publish { .. }, None)
    ));
    // Снимок book_lv2 — переход на быстрый (createTime новее отданного).
    let (act, why) = fb.on_lv2(&snap, 20, stale);
    assert!(matches!(act, Act::Publish { .. }) && why.is_none());
    assert_eq!(fb.stats.to_fast, 1);
    // Быстрый синхронизирован: снимок book ядру не уходит (верх не совпал — не подтверждение и не разрыв). Ревью 424:
    // и второй подряд несовпавший снимок при неизменном быстром — не разрыв, т.к. createTime снимков book примера офдоки
    // раньше createTime быстрого (расхождение объяснимо отставанием book).
    for b in books.drain(..) {
        assert_eq!(fb.on_book(b, 20, stale), (Act::Nothing, None));
    }
    assert_eq!(fb.stats.gaps, 0);
    assert!(matches!(fb.on_lv2(&upd, 20, stale).0, Act::Publish { .. }));
    // Разрыв: страховочный снимок book есть, но его createTime раньше уже отданного — ядру не уходит, пара недостоверна.
    let (act, why) = fb.on_lv2(&upd, 20, stale);
    assert_eq!(act, Act::Invalidate);
    assert!(why.is_some(), "нужна переподписка book_lv2");
    assert_eq!((fb.stats.to_snap, fb.stats.gaps), (1, 1));
    // Срок свежести не задан (venue.book_stale_ms) — страховки нет.
    assert!(
        matches!(fb.on_lv2(&snap, 20, || None), (Act::Nothing, None)),
        "снимок старше отданного"
    );
    let (act, _) = fb.on_lv2(&upd, 20, || None);
    assert!(
        matches!(act, Act::Publish { .. }),
        "после снимка цепочка цела, createTime тот же — отдаётся"
    );
}

#[test]
#[ignore = "записать poloniex_ws_book_lv2.jsonl — кадры book_lv2 wss://ws.poloniex.com/ws/public (docs/TASKS.md)"]
fn recorded_lv2_frames_chain() {
    let t = std::fs::read_to_string("tests/data/cex_samples_2026_11_05/poloniex_ws_book_lv2.jsonl")
        .unwrap();
    let mut books: std::collections::HashMap<String, LocalBook> = Default::default();
    let mut changed = 0;
    for line in t.lines().filter(|l| !l.trim().is_empty()) {
        match depth::parse(line) {
            Msg::Lv2(list) => {
                for r in list {
                    let lb = books.entry(r.symbol.clone()).or_default();
                    match lb.apply(&r) {
                        Applied::Changed => changed += 1,
                        Applied::Waiting => {}
                        Applied::Gap(why) => panic!("разрыв в записи: {why}: {line}"),
                    }
                }
            }
            Msg::BadLv2 { reason, .. } => panic!("кадр не разобран: {reason}: {line}"),
            _ => {}
        }
    }
    assert!(changed > 0);
}
