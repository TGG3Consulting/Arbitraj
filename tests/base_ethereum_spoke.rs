//! Расход кошелька, выплата и защитные отказы по архивам Maverick V1 и Topaz обеих сетей.
use alloy_primitives::U256;
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::BTreeMap;

fn number(v: &Value) -> U256 {
    let s = v.as_str().unwrap();
    if let Some(s) = s.strip_prefix("0x") {
        U256::from_str_radix(s, 16).unwrap()
    } else {
        s.parse().unwrap()
    }
}

#[test]
fn module_router_and_v5_agree_on_wallet_deltas_at_the_same_size() {
    let archives: [(&[u8], usize, usize, usize); 4] = [
        (
            include_bytes!("data/first_run_dex_2026-11-26/maverick_v1_base_wallets.json.gz"),
            231,
            18,
            3,
        ),
        (
            include_bytes!("data/first_run_dex_2026-11-26/maverick_v1_ethereum_wallets.json.gz"),
            150,
            16,
            3,
        ),
        (
            include_bytes!("data/first_run_dex_2026-11-26/topaz_v1_base_wallets.json.gz"),
            8,
            12,
            2,
        ),
        (
            include_bytes!("data/first_run_dex_2026-11-26/topaz_v1_ethereum_wallets.json.gz"),
            2,
            4,
            2,
        ),
    ];
    for (bytes, pools, paid_legs, wires) in archives {
        let d: Value = serde_json::from_reader(GzDecoder::new(bytes)).unwrap();
        assert_eq!(d["selector"]["requireCanonical"], true);
        assert_eq!(d["total"], pools);
        let mut counts = BTreeMap::new();
        for market in d["samples"].as_array().unwrap() {
            let mut payments = BTreeMap::new();
            for c in market["cases"].as_array().unwrap() {
                for side in ["buy", "sell"] {
                    let leg = &c[side];
                    if leg["stage"] != "paid" {
                        continue;
                    }
                    let r = leg["results"].as_array().unwrap();
                    let i = r.len() - 9;
                    assert_eq!(
                        (&r[i]["status"], &r[i + 1]["status"], &r[i + 2]["status"]),
                        (
                            &Value::from("0x0"),
                            &Value::from("0x0"),
                            &Value::from("0x1")
                        )
                    );
                    let pre = i - 7;
                    let input = number(&r[pre]["returnData"]) - number(&r[i + 3]["returnData"]);
                    let received =
                        number(&r[i + 4]["returnData"]) - number(&r[pre + 1]["returnData"]);
                    assert_eq!(input, number(&leg["input"]));
                    assert_eq!(input, number(&leg["paid"]));
                    assert_eq!(received, number(&leg["net"]));
                    for k in 2..6 {
                        assert_eq!(r[pre + k]["returnData"], r[i + 3 + k]["returnData"]);
                    }
                    let key = (side, c["input"].as_str().unwrap());
                    if let Some(prior) = payments.insert(key, (input, received)) {
                        assert_eq!(prior, (input, received));
                    }
                    *counts.entry(c["wire"].as_str().unwrap()).or_insert(0usize) += 1;
                }
            }
        }
        assert_eq!(counts.len(), wires);
        assert!(counts.values().all(|n| *n == paid_legs));
    }
}
