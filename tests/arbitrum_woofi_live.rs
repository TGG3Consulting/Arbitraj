//! Файл: tests/arbitrum_woofi_live.rs
//! Что делает: решение 451 (T448-08) — WOOFi WooPPV2.2 на Arbitrum на записи своего узла: (1) в коде реализации прокси
//! (слот EIP-1967) есть селекторы вызовов, которыми пользуется вид woofi_v2 (query, tryQuery, swap, quoteToken,
//! wooracle, paused), у оракула — state(address); (2) ответ state(base) оракула разбирается ABI вида
//! (abi_woo::IWooracle — 4 слова); (3) wooracle() пула = записанный оракул. Решение 462: оракул записи — wooracle()
//! пула с цепи (20.11.2026: 0xf9b0…8081, не адрес документации 0xCf4E…34Ec — тот пишется рядом для сравнения).
//! Запись второго прогона 20.11.2026 (решение 477, блок 526003986): оракул = wooracle() пула, баз 4 — #[ignore] снят.
//! Кто вызывает: cargo test.
//! От чего зависит: tests/data/arbitrum_woofi_<дата>/ (tools/arbitrum_record/woofi.py), arb::venues::bsc_dex::pools::abi_woo.

use std::path::PathBuf;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, hex};
use serde_json::Value;

use arb::venues::bsc_dex::pools::abi_woo::{IWooPP, IWooracle};

/// Обе официальные пары Base. Одни и те же баланс, код и состояние оракула для квоты и полного execute.
#[test]
fn base_woo_v5_matches_quote_and_wallet_movements() {
    use alloy_primitives::U256;
    use arb::venues::bsc_dex::exec::executor::code::{Version, runtime_sha256_of};
    use sha2::{Digest, Sha256};
    let d: Value = serde_json::from_str(include_str!("data/first_run_dex_2026-11-26/woofi_base.json")).unwrap();
    assert_eq!(d["runtime_file_sha256"].as_str(), runtime_sha256_of(Version::V5));
    let bytes = hex::decode(include_str!("../contracts/out/ArbExecutor.bin-runtime").trim()).unwrap();
    assert_eq!(d["runtime_sha256"], hex::encode(Sha256::digest(bytes)));
    assert_eq!(d["markets"].as_array().unwrap().len(), 2);
    let mut paths = std::collections::BTreeMap::new();
    for c in d["cases"].as_array().unwrap() {
        assert_eq!(c["parent_hash"], d["selector"]["blockHash"]);
        let results = &c["results"];
        let n = |name: &str| results[name]["returnData"].as_str().unwrap().parse::<U256>().unwrap();
        let dx = c["input"].as_str().unwrap().parse::<U256>().unwrap();
        assert_eq!(results["above"]["status"], "0x0");
        assert_eq!(results["exact"]["status"], "0x1");
        assert_eq!(n("in_before") - n("in_after"), dx);
        assert_eq!(n("out_after") - n("out_before"), n("quote"));
        assert_eq!(n("exact"), n("quote"));
        assert!(n("exact") > U256::ZERO);
        assert_eq!((n("executor_in"), n("executor_out")), (U256::ZERO, U256::ZERO));
        let path = (c["source"].as_str().unwrap(), c["target"].as_str().unwrap());
        paths.entry(path).or_insert_with(std::collections::BTreeSet::new).insert(dx);
    }
    assert_eq!(paths.len(), 4);
    assert!(paths.values().all(|v| v.len() == 2));
}

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
    dirs.pop()
        .expect("нет записи: tools/arbitrum_record/woofi.py")
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
fn implementation_has_selectors() {
    let p = rec("proxy.json");
    for (k, v) in p["selectors"].as_object().unwrap() {
        assert_eq!(
            v.as_bool(),
            Some(true),
            "селектора {k} нет в коде реализации"
        );
    }
    assert_eq!(p["oracle_code_has_state"].as_bool(), Some(true));
}

/// state(base) разбирается ABI вида; wooracle() пула = оракул записи.
#[test]
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
