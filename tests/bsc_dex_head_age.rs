//! Файл: tests/bsc_dex_head_age.rs
//! Что делает: решение 121, часть A2 — возраст головы по времени блока. На живых ответах eth_getBlockByNumber
//! 2026-10-25 (tests/data/receipts_2026-10-25/responses.json, узел bsc-dataseed.bnbchain.org): время блока — секунды
//! Unix в поле timestamp; кэш времени Heads не откатывается на старый блок, если ответы пришли не по порядку;
//! возраст записанного блока по часам сервера — от его времени до сейчас.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::node::{heads, head_age}. Моков нет.

use serde_json::Value;

use arb::venues::bsc_dex::node::head_age::age_ms;
use arb::venues::bsc_dex::node::heads::Heads;

/// Записанные заголовки: (номер, время блока в секундах) по порядку номеров.
fn headers() -> Vec<(u64, u64)> {
    let raw = std::fs::read_to_string("tests/data/receipts_2026-10-25/responses.json").unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    let hex =
        |x: &Value| u64::from_str_radix(x.as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
    let mut out: Vec<(u64, u64)> = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["request"]["method"] == "eth_getBlockByNumber")
        .filter_map(|r| r["response"]["result"].as_object().cloned())
        .map(|b| (hex(&b["number"]), hex(&b["timestamp"])))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn block_time_is_unix_seconds_and_cache_keeps_newest() {
    let hs = headers();
    assert!(hs.len() >= 2, "в записи есть заголовки");
    let heads = Heads::default();
    assert_eq!(heads.block_time(), None);
    let (old, new) = (hs[0], hs[hs.len() - 1]);
    // Время записанных блоков — 25.10.2026 в секундах Unix (не миллисекундах).
    assert!((1_790_000_000..1_800_000_000).contains(&new.1), "{new:?}");
    heads.remember_time(new.0, new.1);
    // Ответ о старом блоке пришёл позже — кэш остаётся на новом.
    heads.remember_time(old.0, old.1);
    assert_eq!(heads.block_time(), Some(new));
    // Блок записан в прошлом: возраст по часам сервера больше нуля и не меньше разницы времени двух блоков.
    assert!(age_ms(new.1) > 0);
    assert!(age_ms(old.1) - age_ms(new.1) >= ((new.1 - old.1) * 1000) as i64 - 1000);
}
