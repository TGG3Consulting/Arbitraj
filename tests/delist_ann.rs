//! Файл: tests/delist_ann.rs
//! Что делает: решение 226, уточнение Тиграна 08.11.2026 — ВРЕМЯ делистинга из анонсов и монеты со страницы анонса
//! на ЗАПИСАННЫХ живых данных 08.11.2026 (tests/data/delist_ann_2026-11-08/, откуда и как — README.md там же), без
//! моков:
//! - строгий разбор момента «дата + время + UTC» (gateway/delist_time.rs) — все живые форматы KuCoin, Bybit, OKX,
//!   Bitget, MEXC; дата без времени и время без UTC не считаются;
//! - страницы анонсов (gateway/delist_html.rs): KuCoin «Multiple Tokens» (список «Название (ТИКЕР)», время из
//!   строки про спот), Bybit «Bybit to Delist 1 Token(s)» (HFTUSDT), OKX таблица (у каждой пары своё время), Bitget
//!   (ALPINE/USDT; QUBIC/USDT), пары к другой котируемой монете (USDC, USDⓈ) не берутся;
//! - лента Bybit без тега Spot, анонсы Bitget, сборка записей (delist_ann::items) с прочитанными страницами;
//! - ядро: блок только за delist.block_hours до времени из анонса.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::{delist_time, delist_html, delist_ann}, arb::venues::{kucoin, bybit, okx, bitget},
//! arb::core::delist_check.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use arb::gateway::delist_ann::{self, Known};
use arb::gateway::{delist_html, delist_time};

/// Записанный файл (текст).
fn text(name: &str) -> String {
    std::fs::read_to_string(format!("tests/data/delist_ann_2026-11-08/{name}")).unwrap()
}

/// Записанный файл JSON из папки dir.
fn json(dir: &str, name: &str) -> Value {
    let t = std::fs::read_to_string(format!("tests/data/{dir}/{name}")).unwrap();
    serde_json::from_str(&t).unwrap()
}

/// Момент UTC.
fn at(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap()
}

/// Справочник «тикер → символ».
fn known(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(t, s)| (t.to_string(), s.to_string()))
        .collect()
}

#[test]
fn time_formats_strict() {
    let one = delist_time::one;
    assert_eq!(
        one("removed from the platform at 8:00 on November 5, 2026 (UTC)"),
        Some(at(2026, 11, 5, 8, 0))
    );
    assert_eq!(
        one("will end after Oct 28, 2026, 8:00AM UTC, and all"),
        Some(at(2026, 10, 28, 8, 0))
    );
    assert_eq!(
        one("Delisting 11 Spot trading pairs on 2026-07-28 08:00:00 UTC"),
        Some(at(2026, 7, 28, 8, 0))
    );
    assert_eq!(
        one("DORA/USDT | October 3, 2026, 08:00 - 10:00 UTC"),
        Some(at(2026, 10, 3, 8, 0))
    );
    assert_eq!(
        one(
            "We will delist the above-mentioned trading pairs at 8:00 am UTC on September 18, 2026."
        ),
        Some(at(2026, 9, 18, 8, 0))
    );
    assert_eq!(
        one("suspended from 08:00 UTC on September 30, 2026, for"),
        Some(at(2026, 9, 30, 8, 0))
    );
    assert_eq!(
        one("on November 13, 2026, 10:00 AM (UTC): ALPINE/USDT"),
        Some(at(2026, 11, 13, 10, 0))
    );
    assert_eq!(
        one("Delisting of DOPEUSDT Perpetual Futures Pair [Nov 9, 2026, 12:00 (UTC)]"),
        Some(at(2026, 11, 9, 12, 0))
    );
    assert_eq!(
        one("Nov 15, 2026 10:00 (UTC)"),
        Some(at(2026, 11, 15, 10, 0))
    );
    assert_eq!(
        one("at 1:30 PM UTC on Jan 2, 2027"),
        Some(at(2027, 1, 2, 13, 30))
    );
    // Дата без времени, время без UTC, «on November 13» без года — не время делистинга.
    assert_eq!(one("Delisting of VANRY on Aug 18, 2026"), None);
    assert_eq!(one("KuCoin Will Delist BLAST on 2026-10-16"), None);
    assert_eq!(one("on November 13, 2026, 10:00 AM"), None);
    assert_eq!(one("on November 13"), None);
    // Два разных момента — время не определено однозначно; один и тот же дважды — определено.
    assert_eq!(
        one("at 8:00 on November 3, 2026 (UTC) and 8:00 on November 5, 2026 (UTC)"),
        None
    );
    assert_eq!(
        one("Oct 28, 2026, 8:00AM UTC; Oct 28, 2026, 8:00AM UTC"),
        Some(at(2026, 10, 28, 8, 0))
    );
    // Невозможное время — не момент.
    assert_eq!(one("25:00 on November 5, 2026 (UTC)"), None);
    assert_eq!(one("13:00 PM UTC on Jan 2, 2027"), None);
}

#[test]
fn kucoin_page_multiple_tokens() {
    let lines = delist_html::lines(&text("kucoin_page_multiple_tokens_2026-11-05.html"));
    let k = known(&[
        ("ION", "ION-USDT"),
        ("ATA", "ATA-USDT"),
        ("D", "D-USDT"),
        ("BTC", "BTC-USDT"),
    ]);
    let p = delist_html::pairs(&lines, &k, "USDT");
    let t: Vec<&str> = p.keys().map(String::as_str).collect();
    assert_eq!(t, ["ATA", "D", "ION"]);
    // Время — строки про делистинг/спот (8:00 05.11), не депозит (03.11), не бот (6:00), не вывод (07.12).
    assert!(
        p.values().all(|v| *v == Some(at(2026, 11, 5, 8, 0))),
        "{p:?}"
    );
}

#[test]
fn bybit_pages() {
    let k = known(&[
        ("HFT", "HFTUSDT"),
        ("SWELL", "SWELLUSDT"),
        ("SAND", "SANDUSDT"),
    ]);
    let lines = delist_html::lines(&text("bybit_page_hft.html"));
    let p = delist_html::pairs(&lines, &k, "USDT");
    assert_eq!(p.len(), 1);
    // «Spot: Trading of HFTUSDT pairs … will end after Oct 28, 2026, 8:00AM UTC»; депозит 27.10 и бот — не в счёт.
    assert_eq!(p["HFT"], Some(at(2026, 10, 28, 8, 0)));
    // Снимаются только пары к USDC — USDT-пары SWELL и SAND торгуются дальше.
    let lines = delist_html::lines(&text("bybit_page_swell_pairs.html"));
    assert!(delist_html::pairs(&lines, &k, "USDT").is_empty());
    let p = delist_html::pairs(&lines, &k, "USDC");
    assert_eq!(p["SWELL"], Some(at(2026, 9, 24, 8, 0)));
}

#[test]
fn okx_pages() {
    let k = known(&[
        ("DORA", "DORA-USDT"),
        ("ICX", "ICX-USDT"),
        ("ELF", "ELF-USDT"),
        ("ZEUS", "ZEUS-USDT"),
    ]);
    let lines = delist_html::lines(&text("okx_page_dora_table.html"));
    assert!(lines.contains(&"DORA/USDT | October 3, 2026, 08:00 - 10:00 UTC".to_string()));
    let p = delist_html::pairs(&lines, &k, "USDT");
    // У USDT-пар — 03.10 08:00 (начало окна), а не 30.09 (USDC / EUR пары и Convert).
    assert_eq!(p.len(), 4);
    assert!(
        p.values().all(|v| *v == Some(at(2026, 10, 3, 8, 0))),
        "{p:?}"
    );
    // «selected USDⓈ»: пары к USDⓈ — USDT-пар нет.
    let lines = delist_html::lines(&text("okx_page_selected_usds.html"));
    assert!(delist_html::pairs(&lines, &k, "USDT").is_empty());
}

#[test]
fn bitget_page_and_announcements() {
    let k = known(&[
        ("ALPINE", "ALPINEUSDT"),
        ("QUBIC", "QUBICUSDT"),
        ("BTC", "BTCUSDT"),
    ]);
    let lines = delist_html::lines(&text("bitget_page_alpine_qubic.html"));
    let p = delist_html::pairs(&lines, &k, "USDT");
    let t: Vec<&str> = p.keys().map(String::as_str).collect();
    assert_eq!(t, ["ALPINE", "QUBIC"]);
    // 13.11 10:00 — то же, что offTime справочника (Earn 03:00 и Convert 06.11 — не в счёт).
    assert!(
        p.values().all(|v| *v == Some(at(2026, 11, 13, 10, 0))),
        "{p:?}"
    );
    // Блок «похожие статьи» живой страницы (ссылки /support/articles/…) — не текст анонса: WIN, PUMPBTC, BTW не берутся.
    let full = text("bitget_page_alpine_qubic.html") + &text("bitget_page_related_links.html");
    let k2 = known(&[
        ("ALPINE", "ALPINEUSDT"),
        ("QUBIC", "QUBICUSDT"),
        ("WIN", "WINUSDT"),
        ("PUMPBTC", "PUMPBTCUSDT"),
        ("BTW", "BTWUSDT"),
    ]);
    let p2 = delist_html::pairs(&delist_html::lines(&full), &k2, "USDT");
    assert_eq!(p2, p);
    // Сами ссылки выброшены целиком (в т. ч. «Notice of Delisting 8 Spot Trading Pairs on October 23, 2026»).
    assert!(delist_html::lines(&text("bitget_page_related_links.html")).is_empty());

    let data = json(
        "delist_2026-11-08",
        "bitget_announcements_symbol_delisting.json",
    );
    let (anns, last) = arb::venues::bitget::delist_ann::parse(&data["data"]).unwrap();
    assert_eq!(last.as_deref(), Some("12560603899692"));
    assert_eq!(anns.len(), 1);
    assert!(anns[0].list.is_none() && delist_ann::needs_page(&anns[0]));
    // Со страницей — записи со временем, без неё — предупреждение «монеты не определены».
    let url = anns[0].url.clone().unwrap();
    let pages = HashMap::from([(url.clone(), lines)]);
    let kn = Known {
        map: &k,
        quote: "USDT",
    };
    let since = at(2026, 10, 9, 0, 0);
    let (items, warn) = delist_ann::items(&anns, &pages, &HashMap::new(), &kn, since, "Bitget");
    assert!(warn.is_empty());
    assert_eq!(items.len(), 2);
    assert!(
        items
            .iter()
            .all(|i| i.trading_end == Some(at(2026, 11, 13, 10, 0)) && !i.st)
    );
    assert!(items[0].source.contains("время со страницы анонса"));
    let missed = HashMap::from([(url, "очередь".to_string())]);
    let (items, warn) = delist_ann::items(&anns, &HashMap::new(), &missed, &kn, since, "Bitget");
    assert!(items.is_empty());
    assert!(
        warn[0].contains("монеты не определены — очередь"),
        "{warn:?}"
    );
}

#[test]
fn bybit_feed_without_spot_tag() {
    let data = json(
        "delist_ann_2026-11-08",
        "bybit_announcements_delistings_all.json",
    );
    let (anns, _) = arb::venues::bybit::delist_ann::parse(&data["result"]).unwrap();
    let titles: Vec<&str> = anns.iter().map(|a| a.title.as_str()).collect();
    // Фьючерс (Derivatives) и Bybit Alpha (WEB3) отсеяны; спот без тега Spot — берётся.
    assert_eq!(
        titles,
        [
            "Bybit to Delist 1 Token(s) on Oct 28, 2026",
            "Bybit to Delist 3 Trading Pair(s) on Sep 24, 2026",
            "Delisting of L3,VIC"
        ]
    );
    assert!(anns[0].list.is_none() && delist_ann::needs_page(&anns[0]));
    assert_eq!(anns[2].list.as_deref(), Some("L3,VIC"));
    // Страница HFT прочитана — HFT блокируется за delist.block_hours до 28.10 08:00 UTC.
    let k = known(&[("HFT", "HFTUSDT"), ("L3", "L3USDT")]);
    let url = anns[0].url.clone().unwrap();
    let pages = HashMap::from([(url, delist_html::lines(&text("bybit_page_hft.html")))]);
    let kn = Known {
        map: &k,
        quote: "USDT",
    };
    let since = at(2026, 9, 1, 0, 0);
    let (items, warn) = delist_ann::items(&anns, &pages, &HashMap::new(), &kn, since, "Bybit");
    let t: Vec<&str> = items.iter().map(|i| i.ticker.as_str()).collect();
    assert_eq!(t, ["HFT", "L3"]);
    let hft = &items[0];
    assert_eq!(hft.trading_end, Some(at(2026, 10, 28, 8, 0)));
    // L3: монеты из заголовка, страницы нет — время не определено, блок без времени.
    assert_eq!(items[1].trading_end, None);
    assert!(
        items[1]
            .source
            .contains("время делистинга из анонса не определено")
    );
    // «3 Trading Pair(s)» без страницы — предупреждение.
    assert_eq!(warn.len(), 1, "{warn:?}");

    // Ядро, правило Тиграна (N = 12): за 13 ч до делистинга — торгуем, за 11 ч — нет.
    let h = Decimal::from(12);
    let early = at(2026, 10, 27, 19, 0);
    assert!(arb::core::delist_check::verdict("bybit", hft, false, early, h).is_ok());
    let late = at(2026, 10, 27, 21, 0);
    let why = arb::core::delist_check::verdict("bybit", hft, false, late, h).unwrap_err();
    assert!(why.contains("осталось 11 ч < 12 ч"), "{why}");
}

#[test]
fn kucoin_desc_time_and_page() {
    let data = json("delist_2026-11-08", "kucoin_announcements_delistings.json");
    let (anns, _) = arb::venues::kucoin::delist_ann::parse(&data["data"]).unwrap();
    // «Multiple Tokens»: время есть в annDesc, монет нет — нужна страница.
    let multi = &anns[0];
    assert_eq!(delist_ann::api_time(multi), Some(at(2026, 11, 5, 8, 0)));
    assert!(delist_ann::needs_page(multi));
    // BLAST: монеты и время в API — страница не нужна.
    assert!(!delist_ann::needs_page(&anns[1]));
    let k = known(&[("ION", "ION-USDT"), ("BLAST", "BLAST-USDT")]);
    let url = multi.url.clone().unwrap();
    let lines = delist_html::lines(&text("kucoin_page_multiple_tokens_2026-11-05.html"));
    let pages = HashMap::from([(url, lines)]);
    let kn = Known {
        map: &k,
        quote: "USDT",
    };
    let since = at(2026, 10, 1, 0, 0);
    let (items, warn) = delist_ann::items(&anns, &pages, &HashMap::new(), &kn, since, "KuCoin");
    assert!(warn.is_empty(), "{warn:?}");
    let ion = items.iter().find(|i| i.ticker == "ION").unwrap();
    assert_eq!(ion.trading_end, Some(at(2026, 11, 5, 8, 0)));
    assert!(ion.source.contains("время из текста API анонса"));
    let blast = items.iter().find(|i| i.ticker == "BLAST").unwrap();
    assert_eq!(blast.trading_end, Some(at(2026, 10, 16, 8, 0)));
}
