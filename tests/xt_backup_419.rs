//! Файл: tests/xt_backup_419.rs
//! Что делает: решение 419, дополнение — страховка быстрого стакана XT снимками depth@ на ПРИМЕРАХ ИЗ ОФИЦИАЛЬНОЙ
//! ДОКУМЕНТАЦИИ (github.com/xt-com/xt4-api, исходник doc.xt.com): темы подписки пары (быстрый поток и страховка),
//! разбор примера снимка limitDepth.md (updateId и время t есть) и примера изменений increDepth.md (времени нет —
//! поэтому «не старее отданного» при смене потока держится порядком получения, venues/xt/backup.rs), учёт пары
//! «ядру отдан быстрый стакан»; ревью 419 — сверка верха снимка с быстрым (same_top) и правило «два снимка подряд не
//! равны быстрому, быстрый между ними не менялся» (Backup::observe) на тех же примерах: быстрый стакан — локальный
//! стакан (local_book.rs) с уровнями примера limitDepth.md (равен) или пустой (не равен). Живых записей потоков XT
//! нет (docs/TASKS.md — записать). Моков нет.
//! Кто вызывает: cargo test. От чего зависит: arb::venues::xt (backup, depth, local_book, ws, ws_conn).

use std::time::Duration;

use arb::core::types::PriceLevel;
use arb::venues::xt::backup::{Backup, Snap, same_top};
use arb::venues::xt::depth::{self, Msg};
use arb::venues::xt::local_book::LocalBook;
use arb::venues::xt::ws::{Cfg, Stream};
use arb::venues::xt::ws_conn::topics;

/// Пример кадра «Limited depth» (`_websocket_public/limitDepth.md`), комментарии убраны.
const LIMIT_DOC: &str = r#"{"topic":"depth","event":"depth@btc_usdt,20","data":{"s":"btc_usdt","i":12345678,
"t":1657699200000,"a":[["34000","1.2"],["34001","2.3"]],"b":[["32000","0.2"],["31000","0.5"]]}}"#;
/// Пример кадра «Incremental depth» (`_websocket_public/increDepth.md`), комментарии убраны.
const INCRE_DOC: &str = r#"{"topic":"depth_update","event":"depth_update@btc_usdt","data":{"s":"btc_usdt",
"fi":121,"i":123,"a":[["34000","1.2"],["34001","2.3"]],"b":[["32000","0.2"],["31000","0.5"]]}}"#;

/// Настройки соединения с каналом и страховкой (числа времени в темах не участвуют).
fn cfg(stream: Stream, levels: u64, backup: Option<u64>) -> Cfg {
    Cfg {
        url: String::new(),
        stream,
        levels,
        backup,
        sub_gap: Duration::from_millis(1),
        per_msg: 1,
        ping: Duration::from_millis(1),
        silence: Duration::from_millis(2),
        connect_timeout: Duration::from_millis(1),
        resync_pause: Duration::from_millis(1),
    }
}

#[test]
fn topics_fast_and_backup() {
    let both = topics("btc_usdt", &cfg(Stream::Update, 20, Some(20)));
    assert_eq!(both, vec!["depth_update@btc_usdt", "depth@btc_usdt,20"]);
    let fast = topics("btc_usdt", &cfg(Stream::Update, 30, None));
    assert_eq!(fast, vec!["depth_update@btc_usdt"]);
    let snap = topics("btc_usdt", &cfg(Stream::Depth, 20, None));
    assert_eq!(snap, vec!["depth@btc_usdt,20"]);
}

#[test]
fn doc_snapshot_has_time_update_has_not() {
    let Msg::Snapshot { symbol, data } = depth::parse(LIMIT_DOC) else {
        panic!("пример limitDepth.md не разобран как снимок depth")
    };
    assert_eq!(symbol, "btc_usdt");
    assert_eq!((data.i, data.t), (12345678, Some(1657699200000)));
    let Msg::Update { data, .. } = depth::parse(INCRE_DOC) else {
        panic!("пример increDepth.md не разобран как depth_update")
    };
    assert_eq!((data.fi, data.i, data.t), (Some(121), 123, None));
}

#[test]
fn backup_tracks_fast_pairs() {
    let mut b = Backup::new(true);
    b.given_fast("btc_usdt");
    b.given_fast("btc_usdt");
    assert!(b.forget("btc_usdt"));
    assert!(!b.forget("btc_usdt"));
    let mut off = Backup::new(false);
    off.given_fast("btc_usdt");
    assert!(!off.forget("btc_usdt"));
}

/// Снимок примера limitDepth.md в числах ядра.
fn doc_snap() -> Snap {
    let Msg::Snapshot { data, .. } = depth::parse(LIMIT_DOC) else {
        panic!("пример limitDepth.md не разобран как снимок depth")
    };
    let (bids, asks) = depth::snapshot_levels(&data).expect("уровни примера limitDepth.md");
    Snap {
        bids,
        asks,
        t: data.t,
        i: data.i,
    }
}

/// Верх быстрого стакана: локальный стакан с уровнями того же примера (номер — i примера increDepth.md).
fn doc_fast() -> (Vec<PriceLevel>, Vec<PriceLevel>) {
    let s = doc_snap();
    let mut lb = LocalBook::default();
    lb.snapshot(123, &s.bids, &s.asks);
    lb.top(20)
}

#[test]
fn same_top_compares_common_levels() {
    let s = doc_snap();
    let (fb, fa) = doc_fast();
    assert!(same_top(&s.bids, &fb) && same_top(&s.asks, &fa));
    // Быстрый мельче снимка (снимок REST меньше venue.depth_levels) — общие уровни равны, не расхождение.
    assert!(same_top(&s.asks, &fa[..1]));
    // Сторона быстрого пуста, у снимка нет — расхождение.
    assert!(!same_top(&s.asks, &[]));
    // Другой верх: лучший ask снимка против второго уровня быстрого.
    assert!(!same_top(&s.asks, &fa[1..]));
}

#[test]
fn two_differing_snapshots_in_a_row_mean_divergence() {
    let (fb, fa) = doc_fast();
    let mut b = Backup::new(true);
    b.given_fast("btc_usdt");
    // Равен — не расхождение; не равен один раз — ещё нет; второй подряд — да.
    assert!(!b.observe("btc_usdt", doc_snap(), &fb, &fa));
    assert!(!b.observe("btc_usdt", doc_snap(), &[], &[]));
    assert!(b.observe("btc_usdt", doc_snap(), &[], &[]));
    // Быстрый изменился между снимками — счёт заново, принятый снимок забыт (старее отданного).
    b.given_fast("btc_usdt");
    assert!(b.take_seen("btc_usdt").is_none());
    assert!(!b.observe("btc_usdt", doc_snap(), &[], &[]));
    b.given_fast("btc_usdt");
    assert!(!b.observe("btc_usdt", doc_snap(), &[], &[]));
    // Совпавший снимок между двумя несовпавшими прерывает «подряд».
    assert!(!b.observe("btc_usdt", doc_snap(), &fb, &fa));
    assert!(!b.observe("btc_usdt", doc_snap(), &[], &[]));
    // Снимок, принятый после последнего изменения быстрого, — для отдачи ядру при разрыве.
    let seen = b
        .take_seen("btc_usdt")
        .expect("снимок принят после изменения быстрого");
    assert_eq!((seen.i, seen.t), (12345678, Some(1657699200000)));
    assert!(b.forget("btc_usdt"));
}
