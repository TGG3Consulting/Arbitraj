//! Файл: tests/bingx_pairs_filter.rs
//! Что делает: первичный фильтр монет BingX по справочнику пар и сетей на живых ответах 2026-10-24
//! (tests/data/bingx_2026-10-24: куски common/symbols и config/getall того же запуска):
//! - пары status 1 с apiStateBuy/apiStateSell = false (9 монет с сетью BSC: TAG, TA, B, USD1, SKYAI, RWA, NIULAI, TUT,
//!   XPIN) не входят в торгуемые (SpotPairs.tickers) и отсеиваются причиной NoApiTrading;
//! - пара не status 1 — причина PairNotTrading с кодом как есть (SEA — 10, SOLV — 25);
//! - сеть BEP20 без адреса контракта (BNB — "", ZBC и RAMP — поля нет) — NoContract, а не NoBscNetwork;
//! - монета без BEP20 (PONS) — NoBscNetwork; CAKE со стаканом из записи 2026-10-23 — на бирже есть, обе стороны открыты;
//! - снимок пар сравнивается с прежним (pairs_watch.rs): изменение видно строкой «было → стало».
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bingx (symbols, wallet, pairs, depth), arb::core (registry, market_state). Моков нет.

use std::collections::HashMap;

use alloy_primitives::Address;
use serde_json::Value;

use arb::core::market_state::MarketState;
use arb::core::registry::{self, CoinEntry, Exclusion, Inputs, Presence};
use arb::core::types::{Asset, CoinNetworkInfo, Pool, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::events::MarketEvent;
use arb::venues::bingx::depth::{self, Msg};
use arb::venues::bingx::pairs::Snapshot;
use arb::venues::bingx::{symbols, wallet};

/// Девять монет с BEP20, у которых на 2026-10-24 пара status 1, но apiStateBuy и apiStateSell = false.
const API_CLOSED: [&str; 9] = [
    "B", "NIULAI", "RWA", "SKYAI", "TA", "TAG", "TUT", "USD1", "XPIN",
];

fn bingx() -> VenueId {
    VenueId("bingx".into())
}

fn read(name: &str) -> Value {
    let raw = std::fs::read_to_string(format!("tests/data/bingx_2026-10-24/{name}")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

/// Справочник пар из записанного ответа.
fn syms() -> symbols::Symbols {
    let s = symbols::Symbols::default();
    s.load(symbols::parse_list(&read("symbols.json")["data"], "USDT").unwrap());
    s
}

/// Сети монет из записанного ответа config/getall (сеть BSC у BingX — BEP20).
fn nets() -> Vec<CoinNetworkInfo> {
    wallet::parse_networks(&read("config_getall.json")["data"], "BEP20", &bingx()).unwrap()
}

/// Монеты подписки — как их выбирает ядро (core/subscribe.rs): сеть BSC с адресом контракта.
fn assets(n: &[CoinNetworkInfo]) -> Vec<Asset> {
    let mut a: Vec<Asset> = n
        .iter()
        .filter(|i| i.asset.bsc_contract.is_some())
        .map(|i| i.asset.clone())
        .collect();
    a.sort_by(|x, y| x.ticker.cmp(&y.ticker));
    a
}

/// Стакан CAKE из записи WebSocket 2026-10-23 (tests/data/bingx).
fn cake_book(n: &[CoinNetworkInfo]) -> MarketEvent {
    let raw = std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl").unwrap();
    for line in raw.lines() {
        let m: Value = serde_json::from_str(line).unwrap();
        if let Msg::Depth {
            symbol,
            bids,
            asks,
            exchange_time,
        } = depth::parse(m["m"].as_str().unwrap())
            && symbol == "CAKE-USDT"
        {
            let asset = n.iter().find(|i| i.asset.ticker == "CAKE").unwrap();
            return MarketEvent::Book(depth::to_book(
                asset.asset.clone(),
                bids,
                asks,
                exchange_time,
            ));
        }
    }
    panic!("в записи нет стакана CAKE-USDT")
}

/// Справочник ядра (как core/rebuild.rs) — только биржа: причины отсева у биржи от DEX не зависят.
fn coins(market: &MarketState, n: Vec<CoinNetworkInfo>) -> Vec<CoinEntry> {
    let kinds = vec![(bingx(), VenueKind::Cex)];
    let networks = HashMap::from([(bingx(), n)]);
    let quote = HashMap::from([(bingx(), "USDT".to_string())]);
    let bsc = HashMap::from([(bingx(), "BEP20".to_string())]);
    let intermediate = HashMap::new();
    let pool_ok = |_: &Pool| Ok(());
    registry::build(&Inputs {
        venues: &kinds,
        networks: &networks,
        usdt: Some(Address::ZERO),
        quote: &quote,
        bsc_network: &bsc,
        market,
        pool_ok: &pool_ok,
        intermediate: &intermediate,
    })
}

fn at<'a>(c: &'a [CoinEntry], ticker: &str) -> &'a Presence {
    &c.iter()
        .find(|e| e.asset.ticker == ticker)
        .unwrap()
        .presence[0]
}

#[test]
fn api_closed_pairs_are_not_tradable() {
    let n = nets();
    let snap = Snapshot::build(&syms(), &assets(&n));
    let mut closed = snap.api_closed();
    closed.sort();
    assert_eq!(closed, API_CLOSED);
    for t in API_CLOSED {
        assert!(!snap.tickers.contains(&t.to_string()), "{t}");
        // Стакан у них есть (status 1): цена живая, но торговать через API нельзя.
        assert!(snap.books.iter().any(|(a, _)| a.ticker == t), "{t}");
    }
    assert!(snap.tickers.contains(&"CAKE".to_string()));
    // SEA — status 10: ни стакана, ни торговли; код статуса уходит ядру как есть.
    assert!(snap.missing.contains(&"SEA".to_string()));
    let sea = snap.pairs.iter().find(|p| p.ticker == "SEA").unwrap();
    assert_eq!((sea.status, sea.online), (10, false));
}

#[test]
fn registry_reasons_from_live_responses() {
    let n = nets();
    let mut m = MarketState::default();
    m.apply(Snapshot::build(&syms(), &assets(&n)).event(bingx()));
    m.apply(cake_book(&n));
    let c = coins(&m, n);
    for t in API_CLOSED {
        assert_eq!(at(&c, t).excluded, vec![Exclusion::NoApiTrading], "{t}");
    }
    assert_eq!(at(&c, "SEA").excluded, vec![Exclusion::PairNotTrading(10)]);
    assert_eq!(at(&c, "SOLV").excluded, vec![Exclusion::PairNotTrading(25)]);
    for t in ["BNB", "ZBC", "RAMP"] {
        assert_eq!(at(&c, t).excluded, vec![Exclusion::NoContract], "{t}");
        assert_eq!(at(&c, t).network.as_deref(), Some("BEP20"), "{t}");
    }
    assert_eq!(at(&c, "PONS").excluded, vec![Exclusion::NoBscNetwork]);
    // EGLD: пара и сеть в порядке, стакана в записи нет.
    assert_eq!(at(&c, "EGLD").excluded, vec![Exclusion::NoSpotBook]);
    let cake = at(&c, "CAKE");
    assert!(cake.excluded.is_empty());
    assert!(cake.can_buy && cake.can_sell && cake.can_send && cake.can_receive);
    // Ввод и вывод — по отдельности (живые флаги BEP20): BLUAI — ввод закрыт, XEC — вывод закрыт.
    assert!(!at(&c, "BLUAI").can_receive && at(&c, "BLUAI").can_send);
    assert!(at(&c, "XEC").can_receive && !at(&c, "XEC").can_send);
}

#[test]
fn snapshot_change_is_visible() {
    let n = nets();
    let a = assets(&n);
    let syms = syms();
    let before = Snapshot::build(&syms, &a);
    // Тот же ответ без пары TAG (пара пропала из справочника) — изменение видно, набор стаканов другой.
    let mut v = read("symbols.json");
    v["data"]["symbols"]
        .as_array_mut()
        .unwrap()
        .retain(|s| s["symbol"] != "TAG-USDT");
    syms.load(symbols::parse_list(&v["data"], "USDT").unwrap());
    let after = Snapshot::build(&syms, &a);
    assert_ne!(after.books, before.books);
    assert_eq!(
        after.changes(&before),
        vec!["TAG: API: покупка нет, продажа нет → нет пары"]
    );
    assert!(after.changes(&after).is_empty());
}
