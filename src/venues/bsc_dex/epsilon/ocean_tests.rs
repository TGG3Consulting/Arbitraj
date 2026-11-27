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
    check(raw, [3, 3], 2);
    let prepaid = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/epsilon_openocean_prepaid_base_wallets.json.gz"
    );
    check(prepaid, [4, 4], 0);
}
fn check(raw: &[u8], counts: [usize; 2], missing: usize) {
    let d: Value = serde_json::from_reader(GzDecoder::new(raw)).unwrap();
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
            funding: d
                .get("funding")
                .map(|s| {
                    let role = |address| super::profile::Contract {
                        address,
                        code: B256::ZERO,
                    };
                    super::ocean_funding::Funding {
                        entry: role(addr(s)),
                        beacon: role(Address::ZERO),
                        implementation: role(Address::ZERO),
                        vault: role(
                            "0x8d6fcf96253cd9c828b7c5d37ebcc1e1f0d7aa5e"
                                .parse()
                                .unwrap(),
                        ),
                        vault_implementation: role(Address::ZERO),
                    }
                })
                .map(Box::new),
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
        if let Some(f) = &raw.ocean.as_ref().unwrap().funding
            && raw.ocean.as_ref().unwrap().prepaid(&raw) == Some(true)
        {
            let mut missing = logs.clone();
            missing.retain(|x| x.address() != f.entry.address);
            assert!(proof.amounts(&missing).is_none());
        }
        logs.retain(|x| x.address() != addr(&d["ocean"]));
        assert!(proof.amounts(&logs).is_none());
        assert_eq!(row["controls"].as_array().unwrap().len(), 2);
        forms[usize::from(nested)] += 1;
    }
    assert_eq!(forms, counts);
    assert_eq!(refusals, missing);
}

#[test]
fn production_requests_match_public_price_and_funded_swap() {
    use reqwest::Url;
    use rust_decimal::Decimal;
    use std::collections::BTreeMap;

    let archive: Value = serde_json::from_slice(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/epsilon_openocean_queries_base.json"
    ))
    .unwrap();
    let number = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let address = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let mut forms = [0, 0];
    for row in archive["cases"].as_array().unwrap() {
        if row.get("refused").is_some() {
            continue;
        }
        let nested = row["nested"].as_bool().unwrap();
        let amount = number(&row["n"]);
        let provider_amount = if nested {
            amount - amount * U256::from(archive["fee"].as_u64().unwrap()) / U256::from(10_000)
        } else {
            amount
        };
        let payer = address(&archive[if nested { "router" } else { "wallet" }]);
        for (key, account) in [("quote_url", None), ("swap_url", Some(payer))] {
            let sample = Url::parse(row[key].as_str().unwrap()).unwrap();
            let base = sample.join("./").unwrap();
            let args = |url: &Url| url.query_pairs().into_owned().collect::<BTreeMap<_, _>>();
            let current = super::ocean_api::query(
                &base,
                (address(&row["src"]), address(&row["dst"])),
                provider_amount,
                number(&row["gas_price"]),
                row["slippage"]
                    .as_str()
                    .unwrap()
                    .parse::<Decimal>()
                    .unwrap(),
                account,
            )
            .unwrap();
            assert_eq!(current.path(), sample.path());
            assert_eq!(args(&current), args(&sample));
        }
        assert_eq!(number(&row["spent"]), amount);
        assert!(
            number(&row["quote_out"]) > U256::ZERO
                && number(&row["received"]) >= number(&row["minimum"])
        );
        assert_eq!(row["controls"], serde_json::json!([3, 3]));
        forms[usize::from(nested)] += 1;
    }
    assert_eq!(forms, [3, 4]);
}
