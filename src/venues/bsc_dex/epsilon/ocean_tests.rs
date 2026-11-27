//! Собственный публичный OpenOcean маршрут и тот же источник через Epsilon на одном блоке.
use super::{super::exec::carbon_binding::Code, ocean_wire::Ocean, wire::Plan};
use alloy::rpc::types::Log;
use alloy_primitives::{Address, B256, Bytes, U256, aliases::U24};
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn canonical_api_forms_require_actual_wallet_and_provider_payment() {
    let raw = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/epsilon_openocean_base_wallets.json.gz"
    );
    let d: Value = serde_json::from_reader(GzDecoder::new(raw.as_slice())).unwrap();
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let num = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let owner = addr(&d["owner"]);
    let mut forms = [0, 0];
    let mut refusals = 0;
    for row in d["cases"].as_array().unwrap() {
        let Some(paid) = row.get("exact") else {
            assert!(row["firm"]["refused"].is_string());
            refusals += 1;
            continue;
        };
        let nested = row["nested"].as_bool().unwrap();
        let api = Ocean {
            proxy: addr(&d["ocean"]),
            executor: addr(&d["executor"]),
            implementation: Code {
                shell: B256::ZERO,
                implementation: Address::ZERO,
                runtime: B256::ZERO,
            },
            nested,
            payload: row["firm"]["data"].as_str().unwrap().parse().unwrap(),
        };
        let raw = Plan {
            router: addr(&d["router"]),
            providers: [Address::ZERO; 2],
            pools: [Address::ZERO; 2],
            tiers: [U24::ZERO; 2],
            input: addr(&row["src"]),
            output: addr(&row["dst"]),
            amount: num(&row["n"]),
            minimum: num(&paid["received"]),
            fee_bps: if nested {
                d["fee"].as_u64().unwrap().try_into().unwrap()
            } else {
                0
            },
            first: U256::ZERO,
            deadline: d["ts"].as_u64().unwrap() + 60,
            wallet: Some(addr(&d["wallet"])),
            ocean: Some(api),
        };
        let (proof, body) = raw.signed(8453, owner).unwrap();
        assert_eq!(
            body,
            paid["data"].as_str().unwrap().parse::<Bytes>().unwrap()
        );
        let mut logs = serde_json::from_value::<Vec<Log>>(paid["logs"].clone()).unwrap();
        let correct = (num(&paid["spent"]), num(&paid["received"]));
        assert_eq!(proof.amounts(&logs), Some(correct));
        assert_eq!(correct.0, raw.amount);
        logs.retain(|x| x.address() != addr(&d["ocean"]));
        assert!(proof.amounts(&logs).is_none());
        assert_eq!(row["controls"].as_array().unwrap().len(), 2);
        forms[usize::from(nested)] += 1;
    }
    assert_eq!(forms, [3, 3]);
    assert_eq!(refusals, 2);
}
