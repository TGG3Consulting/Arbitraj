//! Замер полного входа и событий того же рынка, доставленных штатным BookOut / core_out.
//! Время потребителя — контролируемый сценарий; измерения боя и денежные выводы сюда не подставляются.
#[path = "flow_replay/recordings.rs"]
mod recordings;
use arb::core::types::VenueId;
use arb::gateway::{
    core_out,
    core_out_book::{BookOut, Fed},
    core_out_item::{Class, Item, Key},
    events::MarketEvent,
    keep_last::Offer,
};
use arb::settings::{store::Store, value::Scope};
use arb::storage::{db::Db, journal::Journal};
use serde_json::json;
use std::collections::HashMap;
use std::io::Write;
use std::time::Instant;
use tokio::sync::{broadcast, mpsc};

/// Реальный канал tokio, ключи, слияние и доставщик адаптера; выдача пакетами при паузе потребителя.
async fn replay(
    events: &[(i64, MarketEvent)],
    case: &str,
    width: i64,
    cap: usize,
    set: &Store,
    journal: &Journal,
    file: &mut std::fs::File,
) {
    let (tx, mut rx) = mpsc::channel(cap);
    let feed = core_out::of(&tx);
    let venues = ["mexc", "bingx"].map(|v| VenueId(v.into()));
    let books: HashMap<_, _> = venues
        .iter()
        .map(|v| (v.clone(), BookOut::new(&tx, v.clone(), set, journal)))
        .collect();
    let first = events[0].0;
    let mut pos = 0;
    let mut count = 0;
    let started = Instant::now();
    let mut ages = Vec::new();
    while pos < events.len() {
        let start = pos;
        let bucket = if width == 0 {
            0
        } else {
            (events[pos].0 - first) / width
        };
        let drained_at = if width == 0 {
            events[pos].0
        } else {
            first + (bucket + 1) * width
        };
        while pos < events.len()
            && (pos == start || (width > 0 && (events[pos].0 - first) / width == bucket))
        {
            match events[pos].1.clone() {
                MarketEvent::Book(b) => assert!(matches!(
                    books[&b.venue].book(&tx, b),
                    Fed::Sent | Fed::Kept
                )),
                MarketEvent::Pool(p) => {
                    let key = Key::Pool(p.venue.clone(), p.reference.clone());
                    let offer = feed.offer(&tx, key, Item::Event(MarketEvent::Pool(p)), || true);
                    assert!(!matches!(offer, Offer::Lost | Offer::Closed));
                }
                _ => unreachable!(),
            }
            pos += 1;
        }
        // Тест current_thread. До этого места доставщик внутри пакета не выполнялся.
        let expected = rx.len() + feed.waiting();
        for _ in 0..expected {
            let (id, venue, ticker) = match rx.recv().await.unwrap() {
                MarketEvent::Book(b) => (
                    b.received_at.timestamp_nanos_opt().unwrap(),
                    b.venue.0,
                    b.asset.ticker,
                ),
                MarketEvent::Pool(p) => (i64::try_from(p.block).unwrap(), p.venue.0, "SOL".into()),
                _ => unreachable!(),
            };
            let age = drained_at - id;
            assert!(age >= 0);
            ages.push(age);
            count += 1;
            let row = json!({"case":case,"width":width,"capacity":cap,"venue":venue,"ticker":ticker,
                "id":id,"drained_at":drained_at});
            writeln!(file, "{row}").unwrap();
        }
        tokio::task::yield_now().await;
        assert_eq!(feed.waiting(), 0);
    }
    let replaced = venues
        .iter()
        .map(|v| feed.peek(&Class::Book(v.clone())).replaced)
        .sum::<u64>()
        + feed.peek(&Class::Pool(VenueId("bsc_dex".into()))).replaced;
    assert_eq!(events.len() as u64, count + replaced);
    if width == 0 {
        assert_eq!(replaced, 0);
    }
    ages.sort_unstable();
    let summary = json!({"case":case,"width":width,"capacity":cap,"input":events.len(),"delivered":count,"replaced":replaced,
        "age_p50":ages[ages.len()/2],"age_max":ages.last(),"wall_us_with_json_output":started.elapsed().as_micros()});
    writeln!(file, "{}", json!({"summary":summary})).unwrap();
}

/// CI сохраняет идентификаторы доставленных кадров в промежуточный файл для сверки с планами.
#[tokio::test(flavor = "current_thread")]
#[ignore = "полный реплей и метрики ресурса — workflow Recorded market flow"]
async fn recorded_market_flow() {
    let path = std::env::temp_dir().join(format!("arb-flow-replay-{}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    let db = Db::open(&path.join("arb.sqlite")).unwrap();
    let journal = Journal::new(db.clone(), broadcast::channel(64).0);
    let set = Store::load(db, broadcast::channel(64).0, journal.clone())
        .await
        .unwrap();
    for v in ["mexc", "bingx"] {
        set.set(
            "venue.book_events_keep",
            Scope::Venue(v.into()),
            Some(json!(true)),
            "тест",
        )
        .await
        .unwrap();
    }
    let dest = std::env::var_os("FLOW_REPLAY_OUTPUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| path.join("delivery.jsonl"));
    let mut outfile = std::fs::File::create(dest).unwrap();
    for (case, events, profiles) in [
        (
            "archive_books",
            recordings::archive(),
            vec![
                (0, 1),
                (100_000_000, 1),
                (500_000_000, 1),
                (1_993_000_000, 1),
                (1_993_000_000, 8),
            ],
        ),
        (
            "sol_books",
            recordings::sol_books(),
            vec![
                (0, 1),
                (100_000_000, 1),
                (500_000_000, 1),
                (1_993_000_000, 1),
                (1_993_000_000, 8),
            ],
        ),
        (
            "sol_pool_blocks",
            recordings::sol_pool(),
            vec![(0, 1), (4, 1), (32, 1), (32, 8)],
        ),
    ] {
        for (width, cap) in profiles {
            replay(&events, case, width, cap, &set, &journal, &mut outfile).await;
        }
    }
}
