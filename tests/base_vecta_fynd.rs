//! Сопоставляет канонические реестры Vecta/Fynd и реальные дельты кошелька Vecta.
use alloy_primitives::{B256, U256, keccak256};
use serde_json::Value;

fn unzip(bytes: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
}

fn scalar(result: &Value) -> U256 {
    assert_eq!(result["status"], "0x1");
    U256::from_str_radix(
        result["returnData"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap()
}

#[test]
fn all_vecta_markets_and_fee_tiers_keep_their_paid_or_rejected_results() {
    let d: Value = serde_json::from_slice(include_bytes!(
        "data/first_run_dex_2026-11-26/vecta_base_directory.json"
    ))
    .unwrap();
    let w = unzip(include_bytes!(
        "data/first_run_dex_2026-11-26/vecta_base_wallets.json.gz"
    ));
    let t = unzip(include_bytes!(
        "data/first_run_dex_2026-11-26/vecta_base_tiers.json.gz"
    ));
    assert_eq!(d["selector"], w["selector"]);
    assert_eq!(d["selector"], t["selector"]);
    assert_eq!(
        t["tiers"],
        serde_json::json!({"100":1,"350":10,"500":10,"3000":60,"10000":200})
    );
    assert_eq!(t["logs"].as_array().unwrap().len(), 5);
    let pools = d["markets"].as_array().unwrap();
    assert_eq!(pools.len() + 1, d["nonce"].as_u64().unwrap() as usize);
    assert_eq!(pools.len(), 3);
    let mut module = 0;
    let mut router = 0;
    let mut v5 = 0;
    for p in pools {
        let w = w["markets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["pool"] == p["pool"])
            .unwrap();
        for c in w["cases"].as_array().unwrap() {
            if p["liquidity"] == "0" {
                assert_eq!(c["buy"]["stage"], "quote");
                assert!(c["sell"].is_null());
                continue;
            }
            for side in ["buy", "sell"] {
                let leg = &c[side];
                assert_eq!(leg["stage"], "paid");
                assert_eq!(leg["full"], true);
                let rs = leg["results"].as_array().unwrap();
                let n = rs.len();
                assert_eq!(rs[n - 7]["status"], "0x0");
                assert_eq!(rs[n - 6]["status"], "0x0");
                assert_eq!(rs[n - 5]["status"], "0x1");
                let used = scalar(&rs[n - 12]) - scalar(&rs[n - 4]);
                let received = scalar(&rs[n - 3]) - scalar(&rs[n - 11]);
                assert_eq!(used.to_string(), leg["input"]);
                assert_eq!(used.to_string(), leg["paid"]);
                assert_eq!(received.to_string(), leg["net"]);
                assert_eq!(scalar(&rs[n - 10]), scalar(&rs[n - 2]));
                assert_eq!(scalar(&rs[n - 9]), scalar(&rs[n - 1]));
                match c["mode"].as_str().unwrap() {
                    "router" => router += 1,
                    "v5" => v5 += 1,
                    "module" => module += 1,
                    m => panic!("{m}"),
                };
            }
        }
    }
    assert_eq!((router, v5, module), (4, 4, 4));
    assert!(w["module_hash"].as_str().unwrap().parse::<B256>().is_ok());
}

#[test]
fn every_fynd_pool_from_the_core_registry_requires_an_authorized_forward() {
    let direct = keccak256("SwapMustHappenThroughForward()");
    for (data, total) in [
        (
            include_bytes!("data/first_run_dex_2026-11-26/fynd_base_markets.json.gz").as_slice(),
            2,
        ),
        (
            include_bytes!("data/first_run_dex_2026-11-26/fynd_ethereum_markets.json.gz")
                .as_slice(),
            3,
        ),
    ] {
        let d = unzip(data);
        assert_eq!(d["selector"]["requireCanonical"], true);
        assert_eq!(d["unauthenticated_health"]["status"], 401);
        let rows = d["markets"].as_array().unwrap();
        assert_eq!(rows.len(), total);
        for row in rows {
            let hash = row["config"].as_str().unwrap().parse::<B256>().unwrap();
            let encoded =
                alloy_primitives::hex::decode(&row["log"]["data"].as_str().unwrap()[2..]).unwrap();
            assert_eq!(keccak256(&encoded[32..128]).to_string(), row["id"]);
            assert_eq!(
                row["extension"],
                format!("0x{}", alloy_primitives::hex::encode(&hash[..20]))
            );
            assert_eq!(
                row["unsigned_error"]["data"],
                format!("0x{}", alloy_primitives::hex::encode(&direct[..4]))
            );
            assert_eq!(row["unsigned_call"]["to"], row["extension"]);
            assert!(d["deployments"][row["extension"].as_str().unwrap()]["runtime"].is_string());
        }
    }
}
