//! Денежные и отказные пробы штатного DLMM Router, сверка bytes и всех границ с реестром.
use super::umbrae::Path;
use crate::venues::bsc_dex::pools::{family_variant::Variant, umbrae_quotes};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn bytes(v: &Value) -> Bytes {
    v.as_str().unwrap().parse().unwrap()
}
fn num(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn explicit_market_and_size_must_settle_in_full() {
    let d: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/umbrae_dlmm_base_wallets.json.gz"
        )[..],
    ))
    .unwrap();
    let directory: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/umbrae_base_directory.json"
    ))
    .unwrap();
    assert_eq!(directory["selector"], d["selector"]);
    let mut filled = 0;
    let mut partial = 0;
    for m in d["examples"].as_array().unwrap() {
        let market = directory["markets"]["dlmm"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["pool"] == m["pool"])
            .unwrap();
        let path = Path {
            pool: addr(&market["pool"]),
            step: market["step"].as_u64().unwrap().try_into().unwrap(),
        };
        for c in m["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["mode"] == "native")
        {
            for leg in ["buy", "sell"]
                .iter()
                .filter_map(|s| c.get(*s).filter(|v| v.is_object()))
            {
                let calls = leg["calls"].as_array().unwrap();
                let r = leg["results"].as_array().unwrap();
                let n = r.len();
                let qty = num(&leg["input"]);
                let floor = num(&leg["quoted"]);
                let raw = bytes(&r[n - 11]["returnData"]);
                let output = umbrae_quotes::payout(Variant::UmbraeDlmm, &raw, qty);
                assert_eq!(r[n - 10]["status"], "0x0");
                assert_eq!(r[n - 9]["status"], "0x0");
                let data = bytes(&calls[n - 8]["data"]);
                let deadline = U256::from_be_slice(&data[4 + 6 * 32..4 + 7 * 32]);
                let (x, y) = (addr(&calls[n - 7]["to"]), addr(&calls[n - 6]["to"]));
                let owner = addr(&calls[n - 8]["from"]);
                assert_eq!(path.data(x, y, qty, floor, owner, deadline), data);
                assert_eq!(
                    path.data(x, y, qty, floor + U256::ONE, owner, deadline),
                    bytes(&calls[n - 10]["data"])
                );
                if r[n - 8]["status"] == "0x1" {
                    assert_eq!(output, Some(num(&leg["net"])));
                    assert_eq!(num(&leg["paid"]), qty);
                    assert_eq!(
                        qty,
                        num(&r[n - 18]["returnData"]) - num(&r[n - 7]["returnData"])
                    );
                    assert_eq!(
                        floor,
                        num(&r[n - 6]["returnData"]) - num(&r[n - 17]["returnData"])
                    );
                    filled += 1;
                } else if num(&leg["consumed"]) < qty {
                    assert!(output.is_none());
                    partial += 1;
                }
            }
        }
    }
    assert_eq!(filled, 46);
    assert!(partial >= 5);
}
