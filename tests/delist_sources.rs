//! Файл: tests/delist_sources.rs
//! Что делает: решение 226 — источники делистинга бирж на ЗАПИСАННЫХ живых ответах 08.11.2026
//! (tests/data/delist_2026-11-08/, откуда и когда — README.md там же), без моков:
//! - MEXC exchangeInfo: st = true у ASSUSDT (живой случай покупки ASS перед делистингом) → запись без времени;
//! - KuCoin symbols st, Bybit instruments stTag (метка у пары: DYDXUSDC "1", DYDXUSDT "0");
//! - Bitget offTime (ALPINE, QUBIC — 13.11.2026 10:00 UTC), Phemex timeline[3], HTX dl / tca;
//! - OKX instruments (expTime пуст) и анонсы OKX / KuCoin / Bybit: только спотовые и только тикеры из справочника
//!   биржи; фьючерсы отсеяны; «Multiple Tokens», «selected USD», пары «BTC/BRZ» — тикеры только на странице анонса
//!   (страницы и время из анонса — tests/delist_ann.rs);
//! - ядро: запись без времени блокирует (delist_check::verdict), из нескольких записей монеты берётся строже;
//!   метка ST — только при delist.st_block (find(…, st_block)).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::{mexc, kucoin, bybit, bitget, phemex, htx, okx}::delist*, arb::gateway::delist_ann,
//! arb::core::{types, delist_check}.

use std::collections::HashMap;

use chrono::{TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use arb::gateway::delist_ann::{self, Known};

/// Анонсы без прочитанных страниц: записи и предупреждения (страниц нет — как при не заданной
/// delist.ann_html_per_refresh).
fn no_pages(
    anns: &[delist_ann::Ann],
    k: &HashMap<String, String>,
    since: chrono::DateTime<Utc>,
) -> (Vec<arb::core::types::DelistItem>, Vec<String>) {
    let (pages, missed) = (HashMap::new(), HashMap::new());
    let known = Known {
        map: k,
        quote: "USDT",
    };
    delist_ann::items(anns, &pages, &missed, &known, since, "тест")
}

/// Список монет заголовка анонса (пусто — списка нет).
fn lists(anns: &[delist_ann::Ann]) -> Vec<&str> {
    anns.iter()
        .map(|a| a.list.as_deref().unwrap_or(""))
        .collect()
}

/// Записанный файл JSON.
fn load(name: &str) -> Value {
    let text = std::fs::read_to_string(format!("tests/data/delist_2026-11-08/{name}")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Справочник «тикер → символ» из пар.
fn known(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(t, s)| (t.to_string(), s.to_string()))
        .collect()
}

#[test]
fn mexc_st_blocks_ass() {
    let got =
        arb::venues::mexc::delist::parse(&load("mexc_exchange_info_st.json"), "USDT").unwrap();
    let ass = got.find("ASS", true).unwrap();
    assert_eq!(ass.symbol, "ASSUSDT");
    assert_eq!(ass.trading_end, None);
    assert!(ass.st && ass.source.contains("st = true"));
    // Уточнение 08.11: метка ST без delist.st_block = да не блокирует.
    assert!(got.find("ASS", false).is_none());
    assert!(got.find("NTX", true).is_some());
    assert!(got.find("BTC", true).is_none());
    // DOPE с st только у пары к USD1 — к USDT записи нет.
    assert!(got.find("DOPE", true).is_none());
    // Ядро: метка ST при delist.st_block = да — отказ на MEXC при любом пороге.
    let now = Utc.with_ymd_and_hms(2026, 11, 8, 12, 0, 0).unwrap();
    let why =
        arb::core::delist_check::verdict("mexc", ass, true, now, Decimal::from(72)).unwrap_err();
    assert!(why.contains("ASS") && why.contains("метка ST"), "{why}");
}

#[test]
fn kucoin_st_and_announcements() {
    let (got, k) =
        arb::venues::kucoin::delist::parse(&load("kucoin_symbols_st.json")["data"], "USDT")
            .unwrap();
    assert_eq!(got.items.len(), 3);
    assert!(got.find("POLS", true).is_some() && got.find("BTC", true).is_none());
    assert!(got.items.iter().all(|i| i.st) && got.find("POLS", false).is_none());
    assert_eq!(k.get("BTC").map(String::as_str), Some("BTC-USDT"));

    let (anns, total) = arb::venues::kucoin::delist_ann::parse(
        &load("kucoin_announcements_delistings.json")["data"],
    )
    .unwrap();
    assert_eq!(total, 1);
    // Фьючерсный анонс отсеян по типу; спотовые — с вырезанным списком.
    assert_eq!(lists(&anns), ["Multiple Tokens", "BLAST", "VENOM and NFP"]);
    // Сейчас BLAST/VENOM/NFP на KuCoin уже сняты (нет в справочнике 08.11) — для проверки справочник из тех тикеров.
    let since = Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap();
    let k = known(&[
        ("VENOM", "VENOM-USDT"),
        ("NFP", "NFP-USDT"),
        ("BTC", "BTC-USDT"),
    ]);
    let (items, warn) = no_pages(&anns, &k, since);
    let t: Vec<&str> = items.iter().map(|i| i.ticker.as_str()).collect();
    assert_eq!(t, ["VENOM", "NFP"]);
    // Время — из annDesc «at 8:00 on October 14, 2026 (UTC)».
    let at = Utc.with_ymd_and_hms(2026, 10, 14, 8, 0, 0).unwrap();
    assert!(items.iter().all(|i| i.trading_end == Some(at)), "{items:?}");
    // «Multiple Tokens» без страницы — монеты не определены, причина в предупреждениях.
    assert_eq!(warn.len(), 1);
    assert!(warn[0].contains("Multiple Tokens"), "{warn:?}");
    // Анонсы старше глубины не учитываются.
    let late = Utc.with_ymd_and_hms(2026, 10, 15, 0, 0, 0).unwrap();
    assert!(no_pages(&anns, &k, late).0.is_empty());
}

#[test]
fn bybit_st_per_pair_and_announcements() {
    let (got, k) = arb::venues::bybit::delist::parse(
        &load("bybit_instruments_spot_st.json")["result"],
        "USDT",
    )
    .unwrap();
    let t: Vec<&str> = got.items.iter().map(|i| i.ticker.as_str()).collect();
    assert_eq!(t, ["FTT", "CTC"]);
    // DYDX: метка только у пары к USDC.
    assert!(got.find("DYDX", true).is_none() && k.contains_key("DYDX"));
    assert!(got.items.iter().all(|i| i.st));

    let (anns, oldest) =
        arb::venues::bybit::delist_ann::parse(&load("bybit_announcements_spot.json")["result"])
            .unwrap();
    assert_eq!(
        lists(&anns),
        [
            "VANRY",
            "SOLO",
            "ELX, ODOS and DMAIL",
            "BTC/BRZ and USDT/BRZ",
            "leveraged token pairs"
        ]
    );
    assert_eq!(
        oldest,
        Some(Utc.timestamp_millis_opt(1750933231000).unwrap())
    );
    let k = known(&[
        ("VANRY", "VANRYUSDT"),
        ("ODOS", "ODOSUSDT"),
        ("BTC", "BTCUSDT"),
        ("USDT", "USDTX"),
    ]);
    let since = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    let (items, warn) = no_pages(&anns, &k, since);
    let t: Vec<&str> = items.iter().map(|i| i.ticker.as_str()).collect();
    // «BTC/BRZ and USDT/BRZ», «leveraged token pairs» — не тикеры: монеты только на странице (её нет — предупреждение).
    assert_eq!(t, ["VANRY", "ODOS"]);
    assert_eq!(warn.len(), 2, "{warn:?}");
    // «on Aug 18, 2026» — дата без времени: время не определено (страницы нет) — запись без времени.
    assert!(items[0].trading_end.is_none());
    assert!(
        items[0]
            .source
            .contains("время делистинга из анонса не определено")
    );

    // Общая лента delistings: фьючерсы (Derivatives) и залог (Collateral) отсеяны; «Spot Token Delisting Notice» и
    // «Delisting 11 Spot trading pairs on 2026-07-28 08:00:00 UTC» — спот без перечня (монеты — на странице).
    let (anns, _) = arb::venues::bybit::delist_ann::parse(
        &load("bybit_announcements_delistings.json")["result"],
    )
    .unwrap();
    assert_eq!(lists(&anns), ["VANRY", "", ""]);
    assert_eq!(
        delist_ann::api_time(&anns[2]),
        Some(Utc.with_ymd_and_hms(2026, 7, 28, 8, 0, 0).unwrap())
    );
}

#[test]
fn okx_instruments_and_announcements() {
    let (got, k) =
        arb::venues::okx::delist::parse(&load("okx_instruments_spot.json")["data"], "USDT")
            .unwrap();
    assert!(got.items.is_empty() && !got.is_unknown());
    assert_eq!(k.get("BTC").map(String::as_str), Some("BTC-USDT"));

    let (anns, total, oldest) =
        arb::venues::okx::delist_ann::parse(&load("okx_announcements_delistings.json")["data"])
            .unwrap();
    assert_eq!(total, 7);
    assert_eq!(
        oldest,
        Some(Utc.timestamp_millis_opt(1782093610370).unwrap())
    );
    assert_eq!(
        lists(&anns),
        [
            "DORA, ICX, STORJ, ZEUS and ELF",
            "selected USD",
            "selected USDⓈ",
            "GODS, PRCL and DUCK"
        ]
    );
    let k = known(&[
        ("ICX", "ICX-USDT"),
        ("ELF", "ELF-USDT"),
        ("DUCK", "DUCK-USDT"),
        ("USD", "USD-USDT"),
    ]);
    let since = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let (items, _) = no_pages(&anns, &k, since);
    let t: Vec<&str> = items.iter().map(|i| i.ticker.as_str()).collect();
    assert_eq!(t, ["ICX", "ELF", "DUCK"]);
    assert!(items[0].source.contains("OKX to delist DORA"));
    // У OKX время только на странице анонса — без неё запись без времени (блок).
    assert!(items.iter().all(|i| i.trading_end.is_none()));
}

#[test]
fn bitget_off_time() {
    let (got, k) =
        arb::venues::bitget::delist::parse(&load("bitget_spot_symbols.json")["data"], "USDT")
            .unwrap();
    assert!(k.contains_key("BTC"));
    let t: Vec<&str> = got.items.iter().map(|i| i.ticker.as_str()).collect();
    assert_eq!(t, ["ALPINE", "QUBIC"]);
    let at = Utc.with_ymd_and_hms(2026, 11, 13, 10, 0, 0).unwrap();
    assert_eq!(got.find("ALPINE", false).unwrap().trading_end, Some(at));
    // До делистинга 5 дней — при пороге 72 ч сделка разрешена, за 48 ч до него — нет.
    let item = got.find("ALPINE", false).unwrap();
    let early = Utc.with_ymd_and_hms(2026, 11, 8, 12, 0, 0).unwrap();
    assert!(
        arb::core::delist_check::verdict("bitget", item, true, early, Decimal::from(72)).is_ok()
    );
    let close = Utc.with_ymd_and_hms(2026, 11, 11, 12, 0, 0).unwrap();
    assert!(
        arb::core::delist_check::verdict("bitget", item, false, close, Decimal::from(72)).is_err()
    );
}

#[test]
fn phemex_timeline() {
    let got =
        arb::venues::phemex::delist::parse(&load("phemex_products_plus.json")["data"], "USDT")
            .unwrap();
    let t: Vec<&str> = got.items.iter().map(|i| i.ticker.as_str()).collect();
    assert_eq!(t, ["VOXEL", "FIS", "FOXY", "ELX", "SKATE", "PIGGY"]);
    let at = Utc.timestamp_millis_opt(1764928800000).unwrap();
    let voxel = got.find("VOXEL", false).unwrap();
    assert_eq!(voxel.trading_end, Some(at));
    // Снятые (Delisted) и пары без timeline[3] — не в списке.
    assert!(got.find("CTR", false).is_none() && got.find("BTC", false).is_none());
    // Время в прошлом (05.12.2025), пара Listed — блок до явной отмены/переноса (Тигран 08.11; живая проверка: стакан
    // пуст, последняя сделка — в момент делистинга).
    let now = Utc.with_ymd_and_hms(2026, 11, 8, 12, 0, 0).unwrap();
    let why = arb::core::delist_check::verdict("phemex", voxel, false, now, Decimal::from(12))
        .unwrap_err();
    assert!(why.contains("блок до явной отмены/переноса"), "{why}");
}

#[test]
fn htx_dl_tca() {
    let got = arb::venues::htx::delist::parse(&load("htx_v1_common_symbols.json")["data"], "USDT")
        .unwrap();
    let t: Vec<&str> = got.items.iter().map(|i| i.ticker.as_str()).collect();
    assert_eq!(t, ["EURR", "WLFI", "USDR", "VELODROME"]);
    let w = got.find("WLFI", false).unwrap();
    assert_eq!(w.symbol, "wlfiusdt");
    assert_eq!(
        w.trading_end,
        Some(Utc.timestamp_millis_opt(1780664400000).unwrap())
    );
    // dl = false (в т. ч. altusdt с необычным tca) и offline — не в списке.
    assert!(
        got.find("ALT", false).is_none()
            && got.find("C98", false).is_none()
            && got.find("CKB", false).is_none()
    );
}

#[test]
fn strictest_record_wins() {
    let (base, _) =
        arb::venues::bitget::delist::parse(&load("bitget_spot_symbols.json")["data"], "USDT")
            .unwrap();
    let anns = vec![delist_ann::Ann {
        list: Some("ALPINE".into()),
        title: "запись для проверки порядка".into(),
        desc: String::new(),
        url: None,
        at: Utc.with_ymd_and_hms(2026, 11, 6, 0, 0, 0).unwrap(),
    }];
    let k = known(&[("ALPINE", "ALPINEUSDT")]);
    let since = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
    let merged = delist_ann::merge(base, Ok(no_pages(&anns, &k, since)));
    assert_eq!(merged.items.len(), 3);
    // Запись без времени строже записи со временем.
    assert_eq!(merged.find("ALPINE", false).unwrap().trading_end, None);
    // Причина сбоя второстепенного источника — в warnings, пары основного источника остаются.
    let w = delist_ann::merge(merged, Err("анонсы не читаются".into()));
    assert_eq!(w.warnings, ["анонсы не читаются"]);
    assert_eq!(w.items.len(), 3);
}

#[test]
fn tickers_strict() {
    assert_eq!(
        delist_ann::tickers("A1, B and C").unwrap(),
        ["A1", "B", "C"]
    );
    assert_eq!(
        delist_ann::tickers("ELX, ODOS, and DMAIL").unwrap(),
        ["ELX", "ODOS", "DMAIL"]
    );
    assert!(delist_ann::tickers("Multiple Tokens").is_none());
    assert!(delist_ann::tickers("BTC/BRZ").is_none());
    assert!(delist_ann::tickers("123").is_none());
    assert!(delist_ann::tickers("").is_none());
}
