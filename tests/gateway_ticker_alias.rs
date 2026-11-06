//! Файл: tests/gateway_ticker_alias.rs
//! Что проверяет: решение 200 — склейка тикеров одного адреса (gateway/alias_map.rs, aliased_tr.rs) на случае живого
//! журнала 06.11.2026: «У адреса 0x44Ece1031e5b5e2D9169546cC10EA5C95BA96237 разные тикеры: gate=ATEAM,
//! mexc=AMAZINGTEAM». Разбор настройки (верные и неверные элементы), карта биржи (тикер биржи ↔ основной; биржа с тем
//! же тикером — без перевода; конфликт — у биржи другая монета с основным тикером), перевод в обе стороны.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::{alias_map, aliased_tr, events}, arb::core::types.

use alloy_primitives::Address;
use chrono::Utc;

use arb::core::types::{Asset, CoinNetworkInfo, VenueId};
use arb::gateway::alias_map::{self, VenueMap};
use arb::gateway::aliased_tr as tr;
use arb::gateway::events::MarketEvent;

const ADDR: &str = "0x44Ece1031e5b5e2D9169546cC10EA5C95BA96237";

fn net(venue: &str, ticker: &str, addr: Option<Address>, network: &str) -> CoinNetworkInfo {
    CoinNetworkInfo {
        venue: VenueId(venue.into()),
        asset: Asset {
            ticker: ticker.into(),
            bsc_contract: addr,
        },
        network: network.into(),
        deposit_enabled: true,
        withdraw_enabled: true,
        withdraw_fee: None,
        withdraw_min: None,
        withdraw_max: None,
        deposit_min: None,
        deposit_confirmations: None,
        withdraw_precision: None,
        withdraw_step: None,
        updated_at: Utc::now(),
    }
}

fn mexc_map() -> VenueMap {
    let a: Address = ADDR.parse().unwrap();
    let (groups, bad) = alias_map::parse(&[format!("{ADDR}=ATEAM,AMAZINGTEAM")]);
    assert!(bad.is_empty());
    let raw = vec![
        net("mexc", "AMAZINGTEAM", Some(a), "BSC"),
        net("mexc", "AMAZINGTEAM", None, "ETH"),
    ];
    let (m, conflicts) = alias_map::venue_map(&groups, &raw);
    assert!(conflicts.is_empty());
    m
}

#[test]
fn parse_rejects_bad_items() {
    let items = vec![
        format!("{ADDR}=ATEAM,AMAZINGTEAM"),
        "не адрес=A,B".into(),
        format!("{ADDR}=X,Y"),
        "0x44Ece1031e5b5e2D9169546cC10EA5C95BA96238=ONLY".into(),
    ];
    let (groups, bad) = alias_map::parse(&items);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].tickers, vec!["ATEAM", "AMAZINGTEAM"]);
    assert_eq!(bad.len(), 3, "{bad:?}");
}

#[test]
fn venue_map_rename_same_and_conflict() {
    let a: Address = ADDR.parse().unwrap();
    let m = mexc_map();
    assert_eq!(m.out("ATEAM", Some(a)), Some("AMAZINGTEAM"));
    assert_eq!(m.out("ATEAM", None), Some("AMAZINGTEAM"));
    assert_eq!(m.back("AMAZINGTEAM"), Some("ATEAM"));
    // Другой адрес с тем же тикером — без перевода.
    let other: Address = "0x0000000000000000000000000000000000000001"
        .parse()
        .unwrap();
    assert_eq!(m.out("ATEAM", Some(other)), None);
    // Gate уже зовёт монету основным тикером — переводить нечего.
    let (groups, _) = alias_map::parse(&[format!("{ADDR}=ATEAM,AMAZINGTEAM")]);
    let (g, c) = alias_map::venue_map(&groups, &[net("gate", "ATEAM", Some(a), "BSC")]);
    assert!(g.is_empty() && c.is_empty());
    // У биржи есть другая монета ATEAM — склейки нет, конфликт.
    let raw = vec![
        net("x", "AMAZINGTEAM", Some(a), "BSC"),
        net("x", "ATEAM", Some(other), "BSC"),
    ];
    let (x, c) = alias_map::venue_map(&groups, &raw);
    assert!(x.is_empty());
    assert_eq!(c.len(), 1);
}

#[test]
fn translate_both_ways() {
    let a: Address = ADDR.parse().unwrap();
    let m = mexc_map();
    let core = Asset {
        ticker: "ATEAM".into(),
        bsc_contract: Some(a),
    };
    assert_eq!(tr::out_asset(&m, &core).ticker, "AMAZINGTEAM");
    let nets = tr::back_networks(&m, vec![net("mexc", "AMAZINGTEAM", None, "ETH")]);
    assert_eq!(
        nets[0].asset.ticker, "ATEAM",
        "все сети монеты — под основным тикером"
    );
    let e = tr::back_event(
        &m,
        MarketEvent::Invalidated {
            venue: VenueId("mexc".into()),
            ticker: "AMAZINGTEAM".into(),
        },
    );
    assert!(matches!(e, MarketEvent::Invalidated { ticker, .. } if ticker == "ATEAM"));
    // Чужая монета не меняется.
    assert_eq!(tr::back_str(&m, "CAKE"), "CAKE");
}
