//! Первичные события рынка для офлайн-проверки очереди. Окна пересекаются: ключ дедупликации
//! для книги — биржа, тикер и наносекунды получения. Дополнительные window tap не поступают в ядро.
use alloy_primitives::U256;
use arb::core::types::{Asset, OrderBook, Pool, PoolKind, PoolRef, PoolState, PriceLevel, VenueId};
use arb::gateway::events::MarketEvent;
use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

pub const DIR: &str = "tests/data/market_flow_2026-11-23";

#[derive(Deserialize)]
struct RawBook {
    venue: String,
    ticker: String,
    channel: String,
    ns: i64,
    bids: Vec<[Decimal; 2]>,
    best_ask: [Decimal; 2],
}

fn book(raw: RawBook) -> (i64, MarketEvent) {
    let price = |[price, qty]: [Decimal; 2]| PriceLevel { price, qty };
    (
        raw.ns,
        MarketEvent::Book(OrderBook {
            venue: VenueId(raw.venue),
            asset: Asset {
                ticker: raw.ticker,
                contract: None,
                chain: None,
            },
            bids: raw.bids.into_iter().map(price).collect(),
            asks: vec![price(raw.best_ask)],
            exchange_time: None,
            received_at: DateTime::from_timestamp_nanos(raw.ns),
        }),
    )
}

/// Ключ сортируется прежде всего по времени, а не по окну или тикеру.
pub fn archive() -> Vec<(i64, MarketEvent)> {
    let records = GzDecoder::new(
        File::open("tests/data/sigma_windows_2026-11-16/book_frames.jsonl.gz").unwrap(),
    );
    let mut one = BTreeMap::new();
    for line in BufReader::new(records).lines() {
        let row: RawBook = serde_json::from_str(&line.unwrap()).unwrap();
        if row.channel.starts_with("window ") {
            continue;
        }
        let key = (row.ns, row.venue.clone(), row.ticker.clone());
        let e = book(row);
        if let Some((_, MarketEvent::Book(previous))) = one.insert(key, e.clone()) {
            let MarketEvent::Book(current) = e.1 else {
                unreachable!()
            };
            assert_eq!(current.bids, previous.bids);
            assert_eq!(current.asks, previous.asks);
        }
    }
    one.into_values().collect()
}

pub fn sol_books() -> Vec<(i64, MarketEvent)> {
    let data: Value =
        serde_json::from_reader(File::open(format!("{DIR}/sol_book.json")).unwrap()).unwrap();
    serde_json::from_value::<Vec<RawBook>>(data["frames"].clone())
        .unwrap()
        .into_iter()
        .map(book)
        .collect()
}

#[derive(Deserialize)]
struct PoolRow {
    block: u64,
    timestamp: i64,
    sqrt_price_x96: String,
    liquidity: String,
    tick: i32,
}
#[derive(Deserialize)]
struct PoolFile {
    address: String,
    token0: String,
    token1: String,
    fee: u32,
    spacing: i32,
    states: Vec<PoolRow>,
}

/// Версия пула после последнего записанного Swap в каждом блоке. Часы сценария — номер блока.
pub fn sol_pool() -> Vec<(i64, MarketEvent)> {
    let data: PoolFile =
        serde_json::from_reader(File::open(format!("{DIR}/sol_pool.json")).unwrap()).unwrap();
    data.states
        .into_iter()
        .map(|r| {
            let pool = Pool {
                venue: VenueId("bsc_dex".into()),
                family: "unchain-x".into(),
                kind: PoolKind::V3,
                reference: PoolRef::Contract(data.address.parse().unwrap()),
                token0: data.token0.parse().unwrap(),
                token1: data.token1.parse().unwrap(),
                fee_pips: Some(data.fee),
                tick_spacing: Some(data.spacing),
                bin_step: None,
                hooks: None,
                state: PoolState::V3 {
                    sqrt_price_x96: U256::from_str_radix(r.sqrt_price_x96.trim_start_matches("0x"), 16)
                        .unwrap(),
                    liquidity: r.liquidity.parse().unwrap(),
                    tick: r.tick,
                },
                block: r.block,
                received_at: DateTime::<Utc>::from_timestamp(r.timestamp, 0).unwrap(),
            };
            (i64::try_from(r.block).unwrap(), MarketEvent::Pool(pool))
        })
        .collect()
}
