//! Файл: tests/kraken_standby_426.rs
//! Что делает: решение 426 — выбор стакана Kraken между основным соединением и страховкой (venues/kraken/standby.rs) и
//! счётчики журнала venue.book_standby (standby_log.rs) на кадрах ИЗ ОФИЦИАЛЬНОЙ ДОКУМЕНТАЦИИ: снимок и изменение
//! MATIC/USD канала book (https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/book.md, примеры ответа
//! «snapshot» с checksum 2439117997 и «update» с checksum 2114181697, оба с timestamp 2023-10-06T17:35:55.440295Z).
//! Знаки пары для чек-суммы — цена 4, количество 8 (по примеру: обе чек-суммы сходятся при этих знаках — проверено
//! zlib.crc32 по гиду book-checksum-v2). Живых кадров двух соединений нет — записать (docs/TASKS.md, решение 426).
//! Разбор — живой depth::parse, локальный стакан — живой LocalBook. Моков нет.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::kraken::{depth, local_book, standby, standby_log}.

use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use arb::core::types::{Asset, OrderBook, VenueId};
use arb::venues::kraken::depth::{self, Depth, Msg};
use arb::venues::kraken::local_book::{Applied, LocalBook};
use arb::venues::kraken::standby::{Arbiter, Frame, Role, Switch, Ver};
use arb::venues::kraken::standby_log::{Event, Tally};

/// Снимок из документации (book.md, «Snapshot Response»), без изменений.
const SNAP: &str = r#"{"channel":"book","type":"snapshot","data":[{"symbol":"MATIC/USD","bids":[{"price":0.5666,"qty":4831.75496356},{"price":0.5665,"qty":6658.22734739},{"price":0.5664,"qty":18724.91513344},{"price":0.5663,"qty":11563.92544914},{"price":0.5662,"qty":14006.65365711},{"price":0.5661,"qty":17454.85679807},{"price":0.566,"qty":18097.1547},{"price":0.5659,"qty":33644.89175666},{"price":0.5658,"qty":148.3464},{"price":0.5657,"qty":606.70854372}],"asks":[{"price":0.5668,"qty":4410.79769741},{"price":0.5669,"qty":4655.40412487},{"price":0.567,"qty":49844.89424998},{"price":0.5671,"qty":24306.41678},{"price":0.5672,"qty":29783.25223475},{"price":0.5673,"qty":57234.71239278},{"price":0.5674,"qty":45065.04744},{"price":0.5675,"qty":5912.76380354},{"price":0.5676,"qty":42514.92434778},{"price":0.5677,"qty":36304.0847022}],"checksum":2439117997,"timestamp":"2023-10-06T17:35:55.440295Z"}]}"#;
/// Изменение из документации (book.md, «Update Response»), без изменений.
const UPD: &str = r#"{"channel":"book","type":"update","data":[{"symbol":"MATIC/USD","bids":[{"price":0.5657,"qty":1098.3947558}],"asks":[],"checksum":2114181697,"timestamp":"2023-10-06T17:35:55.440295Z"}]}"#;

/// Разбор кадра документации живым depth::parse. Ошибки: паника (пример не разобран).
fn parse(text: &str) -> Depth {
    match depth::parse(text) {
        Msg::Book(mut rows) => rows.remove(0).expect("кадр документации разбирается"),
        _ => panic!("не кадр book"),
    }
}

/// Кадр в локальный стакан; сверен — Frame для выбора, разрыв — None. Ошибок нет.
fn apply(lb: &mut LocalBook, d: &Depth) -> Option<Frame> {
    if let Applied::Gap(_) = lb.apply(d) {
        return None;
    }
    let (bids, asks) = lb.top(10);
    let book = OrderBook {
        venue: VenueId("kraken".into()),
        asset: Asset {
            ticker: "MATIC".into(),
            bsc_contract: None,
        },
        bids,
        asks,
        exchange_time: d.exchange_time,
        received_at: chrono::Utc::now(),
    };
    let ver = Ver {
        ts: d.exchange_time,
        crc: d.checksum?,
    };
    Some(Frame {
        book,
        alive: Arc::new(AtomicI64::new(0)),
        seen_ms: 0,
        ver,
    })
}

/// «Недостоверен» на этом шаге не ждём. Ошибок нет: вызов — паника теста.
fn none() {
    panic!("«недостоверен» здесь не ждём");
}

/// Основной теряет стакан — сразу страховка; основной сверен снова, но его снимок старше отданного (то же время,
/// другая чек-сумма) — не отдаётся; его изменение догнало — снова основной; обе стороны потеряны — «недостоверен».
#[test]
fn standby_covers_main_gap_and_never_goes_back_in_time() {
    let (snap, upd) = (parse(SNAP), parse(UPD));
    assert_eq!(
        snap.exchange_time, upd.exchange_time,
        "в документации у снимка и изменения одно время"
    );
    let arb = Arbiter::default();
    let (mut main, mut spare) = (LocalBook::new(10, 4, 8), LocalBook::new(10, 4, 8));

    let (fed, sw) = arb.offer(
        Role::Main,
        "MATIC",
        apply(&mut main, &snap).expect("чек-сумма снимка"),
        |f| f,
    );
    assert!(
        fed.is_some() && sw.is_none(),
        "первый кадр основного уходит ядру"
    );
    let (fed, _) = arb.offer(
        Role::Standby,
        "MATIC",
        apply(&mut spare, &snap).unwrap(),
        |f| f,
    );
    assert!(
        fed.is_none(),
        "страховка держится в памяти, пока основной сверен"
    );

    let (fed, sw, lost) = arb.lost(Role::Main, "MATIC", |f| f, none);
    let f = fed.expect("основной потерян — сразу стакан страховки");
    assert_eq!(
        (sw, lost, f.ver.crc),
        (Some(Switch::ToStandby), false, 2439117997)
    );
    assert_eq!(f.book.bids.len(), 10);

    let (fed, sw) = arb.offer(
        Role::Standby,
        "MATIC",
        apply(&mut spare, &upd).expect("чек-сумма изменения"),
        |f| f,
    );
    let f = fed.expect("активна страховка — её изменения уходят ядру");
    assert!(sw.is_none());
    assert_eq!(
        f.book.bids[9].qty,
        "1098.3947558".parse::<rust_decimal::Decimal>().unwrap()
    );

    // Основной после переподписки: изменение до снимка — разрыв (живой local_book.rs), неактивная сторона — только забыта.
    let mut main = LocalBook::new(10, 4, 8);
    assert!(apply(&mut main, &upd).is_none());
    assert!(matches!(
        arb.lost(Role::Main, "MATIC", |f| f, none),
        (None, None, false)
    ));
    // Новый снимок основного: то же время, другая чек-сумма — состояние старше отданного, ядру не уходит.
    let (fed, sw) = arb.offer(Role::Main, "MATIC", apply(&mut main, &snap).unwrap(), |f| f);
    assert!(fed.is_none() && sw.is_none());
    // Изменение основного: то же время и та же чек-сумма, что у отданного, — обратно на основной.
    let (fed, sw) = arb.offer(Role::Main, "MATIC", apply(&mut main, &upd).unwrap(), |f| f);
    assert!(fed.is_some());
    assert_eq!(sw, Some(Switch::ToMain));

    assert!(matches!(
        arb.lost(Role::Standby, "MATIC", |f| f, none),
        (None, None, false)
    ));
    let mut blind = false;
    let (fed, sw, lost) = arb.lost(Role::Main, "MATIC", |f| f, || blind = true);
    assert!(
        fed.is_none() && sw.is_none() && lost && blind,
        "обе стороны недостоверны — «недостоверен», как раньше"
    );
}

/// Журнал venue.book_standby: первая запись сразу с числами, повтор внутри интервала — нет, интервал не задан — нет.
#[test]
fn tally_journal_line() {
    let t = Tally::default();
    let m = t
        .note(Event::Switched(Switch::ToStandby), || Some(60))
        .expect("первая запись — сразу");
    assert!(m.contains("с запуска: на страховку 1 (с запуска 1)"), "{m}");
    assert!(
        t.note(Event::Lost(Role::Main), || Some(60)).is_none(),
        "внутри интервала записи нет"
    );
    let quiet = Tally::default();
    assert!(
        quiet.note(Event::Blind, || None).is_none(),
        "интервал не задан — записей нет"
    );
}
