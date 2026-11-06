//! Файл: tests/gateway_ticker_alias_auto.rs
//! Что проверяет: решение 206 — автоматическая склейка тикеров одного адреса (gateway/alias_auto.rs) на случаях
//! аудита живого журнала 06.11.2026 (core.ticker_split: AIO/AIOOLD, SHIB/SHIB3, TST/TSTBSC): правило основного тикера,
//! закрепление основного, пропуск адресов ручной склейки и «Разделить», конфликт у биржи (alias_map::venue_map).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::{alias_auto, alias_map}, arb::core::types.

use alloy_primitives::Address;
use chrono::Utc;

use arb::core::types::{Asset, CoinNetworkInfo, VenueId};
use arb::gateway::alias_auto::{AutoBook, pick_main};
use arb::gateway::alias_map;

const A: &str = "0x44Ece1031e5b5e2D9169546cC10EA5C95BA96237";
const B: &str = "0x2170Ed0880ac9A755fd29B2688956BD959F933F8";

fn net(venue: &str, ticker: &str, addr: Option<Address>) -> CoinNetworkInfo {
    CoinNetworkInfo {
        venue: VenueId(venue.into()),
        asset: Asset {
            ticker: ticker.into(),
            bsc_contract: addr,
        },
        network: "BSC".into(),
        deposit_enabled: true,
        withdraw_enabled: true,
        withdraw_fee: None,
        withdraw_fee_pct: None,
        withdraw_min: None,
        withdraw_max: None,
        deposit_min: None,
        deposit_confirmations: None,
        withdraw_precision: None,
        withdraw_step: None,
        updated_at: Utc::now(),
    }
}

fn c(v: &[(&str, usize)]) -> Vec<(String, usize)> {
    v.iter().map(|(t, n)| (t.to_string(), *n)).collect()
}

#[test]
fn main_ticker_rule() {
    // Большинство бирж сильнее всего.
    assert_eq!(pick_main(&c(&[("AIOOLD", 2), ("AIO", 1)])), "AIOOLD");
    // Равенство: без «OLD», затем не на цифру, затем короче, затем по алфавиту.
    assert_eq!(pick_main(&c(&[("AIOOLD", 1), ("AIO", 1)])), "AIO");
    assert_eq!(pick_main(&c(&[("SHIB3", 1), ("SHIB", 1)])), "SHIB");
    assert_eq!(pick_main(&c(&[("TSTBSC", 1), ("TST", 1)])), "TST");
    assert_eq!(pick_main(&c(&[("LFT", 1), ("LIFEFORM", 1)])), "LFT");
    assert_eq!(pick_main(&[]), "");
}

#[test]
fn groups_sticky_skip_and_conflict() {
    let (a, b): (Address, Address) = (A.parse().unwrap(), B.parse().unwrap());
    let mut book = AutoBook::default();
    book.record(
        "gate",
        &[net("gate", "AIO", Some(a)), net("gate", "SHIB", Some(b))],
    );
    book.record(
        "mexc",
        &[net("mexc", "AIOOLD", Some(a)), net("mexc", "SHIB", Some(b))],
    );
    let g = book.groups(&[]);
    assert_eq!(g.len(), 1, "у SHIB тикер один — склеивать нечего");
    assert_eq!(g[0].tickers, vec!["AIO".to_string(), "AIOOLD".to_string()]);
    // Третья биржа с AIOOLD: большинство за AIOOLD, но основной закреплён — ядро не видит смены имени.
    book.record("bingx", &[net("bingx", "AIOOLD", Some(a))]);
    assert_eq!(book.groups(&[])[0].tickers[0], "AIO");
    // Адрес ручной склейки или «Разделить» — не склеивается автоматически.
    assert!(book.groups(&[a]).is_empty());
    // Конфликт: у mexc есть другая монета AIO (другой адрес) — у mexc склейки нет.
    let mexc = [net("mexc", "AIOOLD", Some(a)), net("mexc", "AIO", Some(b))];
    let (map, conflicts) = alias_map::venue_map(&book.groups(&[]), &mexc);
    assert!(map.is_empty());
    assert_eq!(conflicts.len(), 1);
    // Биржа ушла из справочника — её тикеры убираются из книги.
    book.record("mexc", &[]);
    book.record("bingx", &[]);
    assert!(book.groups(&[]).is_empty());
}
