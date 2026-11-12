//! Файл: tests/bsc_dex_head_age.rs
//! Что делает: решение 121, часть A2 — возраст головы по времени блока; решение 259 — время блока в миллисекундах
//! (секунды timestamp + доля секунды из mixHash, BEP-520). На живых ответах eth_getBlockByNumber 2026-10-25
//! (tests/data/receipts_2026-10-25/responses.json, узел bsc-dataseed.bnbchain.org): в каждом ответе есть и
//! timestamp, и mixHash, и готовое milliTimestamp узла — по нему сверяется block_ms::millis; кэш времени Heads не
//! откатывается на старый блок; секунды (deadline) и миллисекунды (возраст) отдаются раздельно.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::node::{heads, head_age, block_ms}. Моков нет.

use alloy_primitives::B256;
use serde_json::Value;

use arb::venues::bsc_dex::node::block_ms::millis;
use arb::venues::bsc_dex::node::head_age::{BlockTime, age_ms};
use arb::venues::bsc_dex::node::heads::Heads;

/// Записанный заголовок: номер, секунды, mixHash, milliTimestamp узла.
struct Hdr {
    number: u64,
    secs: u64,
    mix: B256,
    milli: u64,
}

/// Записанные заголовки по порядку номеров.
fn headers() -> Vec<Hdr> {
    let raw = std::fs::read_to_string("tests/data/receipts_2026-10-25/responses.json").unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    let hex =
        |x: &Value| u64::from_str_radix(x.as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
    let mut out: Vec<Hdr> = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["request"]["method"] == "eth_getBlockByNumber")
        .filter_map(|r| r["response"]["result"].as_object().cloned())
        .map(|b| Hdr {
            number: hex(&b["number"]),
            secs: hex(&b["timestamp"]),
            mix: b["mixHash"].as_str().unwrap().parse().unwrap(),
            milli: hex(&b["milliTimestamp"]),
        })
        .collect();
    out.sort_by_key(|h| h.number);
    out.dedup_by_key(|h| h.number);
    out
}

#[test]
fn block_time_is_unix_seconds_and_cache_keeps_newest() {
    let hs = headers();
    assert!(hs.len() >= 2, "в записи есть заголовки");
    let heads = Heads::default();
    assert_eq!(heads.block_time(), None);
    assert_eq!(heads.block_time_ms(), None);
    let (old, new) = (&hs[0], &hs[hs.len() - 1]);
    // Время записанных блоков — 25.10.2026 в секундах Unix (не миллисекундах).
    assert!((1_790_000_000..1_800_000_000).contains(&new.secs));
    let bt = |h: &Hdr| BlockTime {
        secs: h.secs,
        millis: millis(h.secs, Some(h.mix)),
    };
    heads.remember_time(new.number, bt(new));
    // Ответ о старом блоке пришёл позже — кэш остаётся на новом.
    heads.remember_time(old.number, bt(old));
    // Секунды (deadline, симуляция) — как раньше; миллисекунды — для возраста.
    assert_eq!(heads.block_time(), Some((new.number, new.secs)));
    assert_eq!(heads.block_time_ms(), Some((new.number, new.milli)));
    // Блок записан в прошлом: возраст по часам сервера больше нуля, разница возрастов = разница времени блоков (±50 мс
    // на ход часов между двумя вызовами).
    let (a_new, a_old) = (age_ms(new.milli), age_ms(old.milli));
    assert!(a_new > 0);
    let d = (new.milli - old.milli) as i64;
    assert!((a_old - a_new - d).abs() <= 50, "{a_old} {a_new} {d}");
}

#[test]
fn millis_matches_node_milli_timestamp() {
    // Все записанные блоки: timestamp·1000 + mixHash = milliTimestamp узла (BEP-520), доля секунды < 1000.
    for h in headers() {
        assert_eq!(millis(h.secs, Some(h.mix)), h.milli, "блок {}", h.number);
        assert!(h.milli - h.secs * 1000 < 1000);
    }
    // Живой ответ 12.11.2026 (публичный RPC BNB Chain, блок 132983865): timestamp 0x6af47ad0, mixHash …0384,
    // milliTimestamp 0x1a1caffc004.
    let mix: B256 = "0x0000000000000000000000000000000000000000000000000000000000000384"
        .parse()
        .unwrap();
    assert_eq!(millis(0x6af4_7ad0, Some(mix)), 0x1a1_caff_c004);
}

#[test]
fn millis_without_valid_mix_hash_is_seconds() {
    // Поля нет — как раньше, по секундам (возраст не занижается).
    assert_eq!(millis(1_794_407_120, None), 1_794_407_120_000);
    // mixHash не миллисекунды (блок до Lorentz — произвольный хеш, значение ≥ 1000) — доля 0.
    let big: B256 = "0x00000000000000000000000000000000000000000000000000000000000003e8"
        .parse()
        .unwrap();
    assert_eq!(millis(1_794_407_120, Some(big)), 1_794_407_120_000);
    let random: B256 = "0x9a1f00000000000000000000000000000000000000000000000000000000beef"
        .parse()
        .unwrap();
    assert_eq!(millis(1_794_407_120, Some(random)), 1_794_407_120_000);
}
