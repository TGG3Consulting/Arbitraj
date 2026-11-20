//! Файл: tests/arbitrum_woofi_live.rs
//! Что делает: решение 451 (T448-08) — WOOFi WooPPV2.2 на Arbitrum на записи своего узла: (1) в коде реализации прокси
//! (слот EIP-1967) есть селекторы вызовов, которыми пользуется вид woofi_v2 (query, tryQuery, swap, quoteToken,
//! wooracle, paused), у оракула — state(address); (2) ответ state(base) оракула WooracleV2_2 (Sourcify 42161 exact
//! match, contracts/wooracle/WooracleV2_2.sol) разбирается ABI вида (abi_woo::IWooracle — 4 слова); (3) wooracle()
//! пула = записанный оракул.
//! Кто вызывает: cargo test -- --ignored (после записи).
//! От чего зависит: tests/data/arbitrum_woofi_<дата>/ (tools/arbitrum_record/woofi.py), arb::venues::bsc_dex::pools::abi_woo.

use std::path::PathBuf;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, hex};
use serde_json::Value;

use arb::venues::bsc_dex::pools::abi_woo::{IWooPP, IWooracle};

/// Последняя по имени папка записи tests/data/arbitrum_woofi_*. Паника — записи нет.
fn data_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|d| d.ok().map(|d| d.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_woofi_"))
        })
        .collect();
    dirs.sort();
    dirs.pop().expect("нет записи: tools/arbitrum_record/woofi.py")
}

fn rec(name: &str) -> Value {
    let text = std::fs::read_to_string(data_dir().join(name)).expect(name);
    serde_json::from_str(&text).expect("json")
}

/// Байты ответа {"ok": "0x…"}; откат — паника с текстом.
fn ok(v: &Value) -> Vec<u8> {
    hex::decode(v["ok"].as_str().expect("ответ")).expect("hex")
}

/// Селекторы вида есть в коде реализации прокси и оракула.
#[test]
#[ignore = "запись 2026-11-20: wooracle() пула = 0xf9b0c8ee…, а оракул записи 0xCf4EA168…; bases пуст (record_all без --base) — повтор woofi.py (TASKS T456-03)"]
fn implementation_has_selectors() {
    let p = rec("proxy.json");
    for (k, v) in p["selectors"].as_object().unwrap() {
        assert_eq!(v.as_bool(), Some(true), "селектора {k} нет в коде реализации");
    }
    assert_eq!(p["oracle_code_has_state"].as_bool(), Some(true));
}

/// state(base) разбирается ABI вида; wooracle() пула = оракул записи.
#[test]
#[ignore = "запись 2026-11-20: wooracle() пула = 0xf9b0c8ee…, а оракул записи 0xCf4EA168…; bases пуст (record_all без --base) — повтор woofi.py (TASKS T456-03)"]
fn oracle_state_decodes() {
    let meta = rec("meta.json");
    let pool = rec("pool.json");
    let oracle: Address = meta["oracle"].as_str().unwrap().parse().unwrap();
    let got = IWooPP::wooracleCall::abi_decode_returns(&ok(&pool["wooracle"])).unwrap();
    assert_eq!(got, oracle);
    let mut priced = 0;
    for b in pool["bases"].as_array().unwrap() {
        let raw = ok(&b["state"]);
        assert_eq!(raw.len(), 4 * 32, "state({}) — не 4 слова", b["base"]);
        let s = IWooracle::stateCall::abi_decode_returns(&raw).unwrap();
        if s.price > 0 && s.woFeasible {
            priced += 1;
        }
    }
    assert!(priced > 0, "ни у одной монеты нет цены оракула");
}
