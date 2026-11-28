//! Регрессия Spectra/IPOR на Base. Сверяет платежи до и после разблокировки по записанным RPC-ответам.
//! Выполняется обычной матрицей GitHub Actions. Исходные квитанции собирает record_spectra_ipor.py.
use alloy_primitives::U256;
use serde_json::Value;
use std::collections::BTreeSet;

/// Читает uint256 успешного balanceOf, get_dy или getAccountLockTime.
fn value(record: &Value) -> U256 {
    let result = &record["result"];
    assert_eq!(result["status"], "0x1");
    let raw = result["returnData"].as_str().unwrap().strip_prefix("0x").unwrap();
    U256::from_str_radix(raw, 16).unwrap()
}

#[test]
fn preheld_and_unlocked_shares_settle_both_directions_at_two_sizes() {
    let archive: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
        &include_bytes!("data/first_run_dex_2026-11-26/spectra_base_ipor_delayed.json.gz")[..]
    )).unwrap();
    let mut cases = BTreeSet::new();
    for s in archive["scenarios"].as_array().unwrap() {
        let r = &s["records"];
        let h = &s["headers"];
        let mode = s["mode"].as_str().unwrap();
        let funding = s["funding"].as_str().unwrap();
        let raw = s["raw_ibt"].as_str().unwrap();
        assert!(cases.insert((mode, funding, raw)));
        assert_eq!(h[0]["parentHash"], archive["parent"]["blockHash"]);
        assert_eq!(h[1]["parentHash"], h[0]["hash"]);
        if funding == "deposit" {
            assert_eq!(r["early_exchange"]["result"]["status"], "0x0");
            let unlock = value(&r["lock_after"]);
            let time = |i: usize| U256::from_str_radix(h[i]["timestamp"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
            assert!(time(0) < unlock && time(1) >= unlock);
        } else {
            assert_eq!(funding, "owned_ibt");
            assert!(value(&r["lock_before"]).is_zero());
        }
        let spent = value(&r["pre_ibt"]) - value(&r["forward_in"]);
        let pt = value(&r["forward_out"]) - value(&r["pre_pt"]);
        let back = value(&r["reverse_out"]) - value(&r["forward_in"]);
        assert_eq!(spent.to_string(), raw);
        assert_eq!(pt, value(&r["quote"]));
        assert_eq!(pt, value(&r["forward_out"]) - value(&r["reverse_in"]));
        assert_eq!(back, value(&r["reverse_quote"]));
        assert!(pt > U256::ZERO && back > U256::ZERO);
        for stage in ["forward", "reverse"] {
            assert_eq!(r[stage]["result"]["status"], "0x1");
            assert_eq!(r[format!("{stage}_strict")]["result"]["status"], "0x0");
            if mode != "pool" {
                assert_eq!(r[format!("{stage}_expired")]["result"]["status"], "0x0");
            }
        }
    }
    assert_eq!(cases.len(), 12);
}
