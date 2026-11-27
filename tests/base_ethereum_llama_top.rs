//! Двадцатки DefiLlama сопоставляются по сети и физической кассе. API-агрегат не подменяет адресный домен.
use serde_json::Value;
use std::collections::BTreeSet;

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}
fn items(value: &Value) -> &[Value] {
    value.as_array().unwrap()
}

#[test]
fn both_rankings_bind_the_exact_profile_without_turning_an_external_volume_into_a_pool() {
    let map = json(include_str!(
        "data/first_run_dex_2026-11-26/llama_top_bindings.json"
    ));
    let ranks = json(include_str!(
        "data/first_run_dex_2026-11-26/catalog_recheck.json"
    ));
    let profiles = json(include_str!("../config/first_run_dex.json"));
    let bytes = include_bytes!("data/first_run_dex_2026-11-26/llama_top_contracts.json.gz");
    let chains: Vec<Value> =
        serde_json::from_reader(flate2::read::GzDecoder::new(&bytes[..])).unwrap();
    assert_eq!(map["revision"], "be9967c3f0d3ccd06c871bab0d16beb3304de719");
    for chain in &chains {
        let net = chain["network"].as_str().unwrap();
        let entries = items(&map["networks"][net]["rows"]);
        let volumes = &ranks[net]["volume"]["by_id"];
        let mut all: Vec<_> = volumes.as_object().unwrap().iter().collect();
        all.sort_by(|a, b| b.1.as_f64().unwrap().total_cmp(&a.1.as_f64().unwrap()));
        assert_eq!(entries.len(), 20);
        for (i, r) in entries.iter().enumerate() {
            let id = r["id"].as_str().unwrap();
            assert_eq!(all[i].0, id);
            assert_eq!(all[i].1, &r["usd24h"]);
            assert_eq!(r["rank"].as_u64(), Some((i + 1) as u64));
            for d in items(&r["domains"]) {
                let matches = items(&profiles)
                    .iter()
                    .filter(|p| p["id"] == d["profile"] && p["network"] == net)
                    .collect::<Vec<_>>();
                assert_eq!(matches.len(), 1);
                let value = &matches[0]["settings"][d["setting"].as_str().unwrap()];
                assert_eq!(
                    value.as_str().unwrap().to_lowercase(),
                    d["address"].as_str().unwrap()
                );
            }
        }
        let row = |id: &str| entries.iter().find(|r| r["id"] == id).unwrap();
        assert_eq!(row("2198")["kind"], "dataset");
        assert_eq!(row("2198")["domains"][0]["literal_in_adapter"], false);
        let v4 = &chain["v4"];
        assert_eq!(v4["manager"], row("5690")["domains"][0]["address"]);
        assert_eq!(v4["sample"]["address"], v4["manager"]);
        assert_eq!(v4["sample"]["topics"][0], v4["topic"]);
        let fluid = &chain["fluid"];
        let advertised: BTreeSet<_> = items(&fluid["resolver_pools"])
            .iter()
            .map(|p| p[0].as_str().unwrap())
            .collect();
        let registry: BTreeSet<_> = items(&fluid["factory_pools"])
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect();
        assert_eq!(advertised, registry);
        assert_eq!(fluid["factory"], row("5317")["domains"][0]["address"]);
        for sample in items(&chain["registered_cl"]) {
            let profile = items(&profiles)
                .iter()
                .find(|p| p["network"] == net && p["id"] == sample["profile"])
                .unwrap();
            assert_eq!(
                profile["settings"]["dex.factory"]
                    .as_str()
                    .unwrap()
                    .to_lowercase(),
                sample["factory"].as_str().unwrap()
            );
            if !sample["sample"].is_null() {
                assert_eq!(sample["sample"]["address"], sample["pool"]);
                assert_eq!(sample["sample"]["topics"][0], sample["swap_topic"]);
            }
        }
        if net == "base" {
            let early = &chain["early_slipstream"];
            assert_eq!(row("4524")["other_source_addresses"][0], early["factory"]);
            assert_eq!(items(&row("4524")["domains"]).len(), 4);
            assert!(
                items(&row("4524")["domains"])
                    .iter()
                    .all(|d| d["address"] != row("8691")["domains"][0]["address"])
            );
            assert_eq!(items(&early["pools"]).len(), 22);
            assert_eq!(
                items(&early["pools"])
                    .iter()
                    .filter(|p| p["liquidity"] != "0")
                    .count(),
                2
            );
            assert_eq!(early["last24h_swaps"], 1);
            assert!(
                items(&early["pools"])
                    .iter()
                    .any(|p| p["pool"] == early["sample"]["address"])
            );
            assert_eq!(row("8180")["kind"], "card");
            assert_eq!(row("8849")["kind"], "shared");
        } else {
            let k = &chain["kipseli"];
            assert_eq!(k["address"], row("7984")["other_source_addresses"][0]);
            assert_eq!(k["sample"]["address"], k["address"]);
            let legs = items(&k["transfers"]);
            assert_eq!(legs.len(), 2);
            assert_eq!(legs[0]["address"], k["token_in"]);
            assert_eq!(legs[1]["address"], k["token_out"]);
            let endpoint = |topic: &Value| format!("0x{}", &topic.as_str().unwrap()[26..]);
            assert_eq!(
                endpoint(&legs[0]["topics"][2]),
                k["wallet"].as_str().unwrap()
            );
            assert_eq!(
                endpoint(&legs[1]["topics"][1]),
                k["wallet"].as_str().unwrap()
            );
            assert_eq!(
                endpoint(&legs[1]["topics"][2]),
                k["recipient"].as_str().unwrap()
            );
            for (leg, field) in legs.iter().zip(["amount_in", "amount_out"]) {
                let amount = u128::from_str_radix(&leg["data"].as_str().unwrap()[2..], 16).unwrap();
                assert_eq!(amount.to_string(), k[field].as_str().unwrap());
            }
            assert_eq!(row("6650")["kind"], "issuance");
            assert_eq!(row("2116")["kind"], "rfq");
        }
    }
}

#[test]
fn a_mixed_0x_transaction_attributes_only_the_two_sided_rfq_leg_to_its_maker() {
    let bytes = include_bytes!("data/first_run_dex_2026-11-26/llama_top_0x_rfq.json.gz");
    let sample: Value = serde_json::from_reader(flate2::read::GzDecoder::new(&bytes[..])).unwrap();
    let receipt = &sample["receipt"];
    let legs = items(&receipt["logs"]);
    let marker = &sample["marker"];
    let input = &sample["maker_input"];
    let output = &sample["maker_output"];
    assert_eq!(sample["registration"]["address"], sample["registry"]);
    assert_eq!(sample["transaction"]["hash"], receipt["transactionHash"]);
    assert_eq!(sample["transaction"]["blockHash"], receipt["blockHash"]);
    assert_eq!(marker["address"], sample["settler"]);
    assert!(items(&marker["topics"]).is_empty());
    assert_eq!(marker["data"].as_str().unwrap().len(), 98);
    let recipient = |log: &Value| format!("0x{}", &log["topics"][2].as_str().unwrap()[26..]);
    let payer = |log: &Value| format!("0x{}", &log["topics"][1].as_str().unwrap()[26..]);
    assert_eq!(
        recipient(&sample["registration"]),
        sample["settler"].as_str().unwrap()
    );
    assert_eq!(recipient(input), sample["maker"].as_str().unwrap());
    assert_eq!(payer(output), sample["maker"].as_str().unwrap());
    assert_eq!(payer(input), sample["settler"].as_str().unwrap());
    assert_eq!(recipient(output), sample["settler"].as_str().unwrap());
    assert_ne!(input["address"], output["address"]);
    assert_eq!(
        &marker["data"].as_str().unwrap()[66..],
        &output["data"].as_str().unwrap()[34..]
    );
    for included in [marker, input, output] {
        assert_eq!(legs.iter().filter(|log| *log == included).count(), 1);
    }
    assert_eq!(
        legs.iter()
            .filter(
                |log| log["address"] == "0x000000000004444c5dc75cb358380d2e3de08a90"
                    && log["topics"][0]
                        .as_str()
                        .is_some_and(|t| t.starts_with("0x40e9cecb"))
            )
            .count(),
        2
    );
}

#[test]
fn coverage_preserves_each_catalog_row_and_distinguishes_missing_from_zero_volume() {
    let document = include_str!("../docs/BASE_ETHEREUM_SETTINGS.md");
    let snapshot = json(include_str!(
        "data/first_run_dex_2026-11-26/catalog_recheck.json"
    ));
    for (key, heading, catalog_rows, expected_known) in [
        ("base", "### Base · DEX", 211, 191),
        ("ethereum", "### Ethereum · DEX", 193, 168),
    ] {
        let network = document.split_once(heading).unwrap().1;
        let table = network
            .split_once("#### DefiLlama")
            .unwrap()
            .1
            .split_once("| ID | Площадка | 24 ч, USD |")
            .unwrap()
            .1;
        let volumes = snapshot[key]["volume"]["by_id"].as_object().unwrap();
        let mut ids = BTreeSet::new();
        let mut known = 0;
        let mut nonspot = 0;
        for row in table
            .lines()
            .skip(2)
            .take_while(|line| line.starts_with("| "))
        {
            let mut cells = row.split('|').skip(1).map(str::trim);
            let id = cells.next().unwrap();
            assert!(id.parse::<u64>().is_ok() && ids.insert(id), "{key} {id}");
            let volume = cells.nth(1).unwrap();
            let decision = row.rsplit('|').nth(1).unwrap().trim();
            if decision.starts_with("нет спота") {
                nonspot += 1;
                assert!(decision.contains("https://") && decision.contains("#L"), "{key} {id}");
            }
            match (key, id) {
                (_, "3308" | "7520") => assert!(decision.starts_with("нет спота"), "{key} {id}"),
                ("base", "5662") => assert!(decision.contains("Spot Swap")),
                ("ethereum", "483") => assert!(decision.contains("vToken/WETH")),
                ("ethereum", "4197") => assert!(decision.contains("ERC20")),
                ("ethereum", "8464") => assert!(decision.contains("Uniswap")),
                _ => {}
            }
            if let Some(number) = volumes.get(id).and_then(Value::as_f64) {
                known += 1;
                assert!(volume.parse::<f64>().is_ok(), "{key} {id}: {volume}");
                if row.contains("оборот < 5 000 USD, отложено") {
                    assert!(number < 5_000.0, "{key} {id}: {number}");
                }
            } else {
                assert_eq!(volume, "нет данных", "{key} {id}");
            }
        }
        assert_eq!((ids.len(), known), (catalog_rows, expected_known));
        assert_eq!(nonspot, if key == "base" { 39 } else { 21 });
    }
}
