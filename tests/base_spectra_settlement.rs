//! Пересчитывает списание и выплату с архивных балансов для каждого полного обмена Spectra.
use alloy_primitives::U256;
use serde_json::Value;
use std::collections::BTreeSet;

fn value(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn balance(result: &Value) -> U256 {
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

/// Возвращает число подтверждённых ног по каналу и покрытие рынков после сверки движения токенов.
fn settlement_counts(bytes: &[u8]) -> (usize, usize, usize, usize, usize, usize) {
    let archive: Value = serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap();
    let mut v5 = 0;
    let mut direct = 0;
    let mut router = 0;
    let mut templates = BTreeSet::new();
    let mut markets = BTreeSet::new();
    let mut matured = BTreeSet::new();
    for m in archive["markets"].as_array().unwrap() {
        for wallet in m["wallets"].as_array().unwrap() {
            let Some(legs) = wallet["legs"].as_array() else {
                continue;
            };
            for leg in legs.iter().filter(|l| l["stage"] == "accepted") {
                let mode = wallet["mode"].as_str().unwrap();
                let results = leg["results"].as_array().unwrap();
                let calls = leg["calls"].as_array().unwrap();
                let real = results.len() - 5;
                let controls = if mode == "pool" { 1 } else { 2 };
                let before = real - controls - 2;
                assert_eq!(results[real]["status"], "0x1");
                assert!(
                    results[before + 2..real]
                        .iter()
                        .all(|r| r["status"] == "0x0")
                );
                assert_eq!(calls[real]["from"], wallet["owner"]);
                assert_eq!(
                    balance(&results[before]) - balance(&results[real + 1]),
                    value(&leg["qty"])
                );
                assert_eq!(
                    balance(&results[real + 2]) - balance(&results[before + 1]),
                    value(&leg["gross"])
                );
                assert_eq!(leg["qty"], leg["paid"]);
                assert_eq!(leg["net"], leg["gross"]);
                if mode == "v5" {
                    assert_eq!(balance(&results[real + 3]), U256::ZERO);
                    assert_eq!(balance(&results[real + 4]), U256::ZERO);
                    v5 += 1;
                } else if mode == "pool" {
                    direct += 1;
                } else if mode == "router" {
                    router += 1;
                }
                templates.insert(m["template"].as_str().unwrap());
                markets.insert(m["pool"].as_str().unwrap());
                if m["maturity"].as_u64().unwrap() <= 1_795_642_926 {
                    matured.insert(m["pool"].as_str().unwrap());
                }
            }
        }
    }
    (v5, direct, router, templates.len(), markets.len(), matured.len())
}

#[test]
fn both_wallet_assets_and_strict_controls_match_in_live_and_matured_markets() {
    let primary = settlement_counts(include_bytes!("data/first_run_dex_2026-11-26/spectra_base_wallets.json.gz"));
    assert_eq!(primary, (96, 96, 96, 4, 24, 18));
    let extra = settlement_counts(include_bytes!("data/first_run_dex_2026-11-26/spectra_base_extra_ibt.json.gz"));
    assert_eq!((extra.0, extra.1, extra.2, extra.4, extra.5), (32, 32, 32, 8, 7));
    let small = settlement_counts(include_bytes!("data/first_run_dex_2026-11-26/spectra_base_small_ibt.json.gz"));
    assert_eq!((small.0, small.1, small.2, small.4), (4, 4, 4, 2));
}

/// Проверяет границу поиска в поставляемой конфигурации по физическим парам каждой фабрики.
#[test]
fn every_registered_pair_fits_the_first_run_lookup_and_has_a_known_template() {
    let bytes = include_bytes!("data/first_run_dex_2026-11-26/spectra_base_directory.json.gz");
    let dir: Value = serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
    let source: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/spectra_base_sources.json"
    )).unwrap();
    let profiles: Value = serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let group = profiles.as_array().unwrap().iter().find(|p| p["id"] == "spectra-v2-base").unwrap();
    let children = group["settings"]["dex.sub_dex"].as_array().unwrap();
    assert_eq!(source["selector"], dir["selector"]);
    let mut pools = BTreeSet::new();
    let mut pairs = std::collections::BTreeMap::new();
    let mut templates = BTreeSet::new();
    for profile in profiles.as_array().unwrap().iter().filter(|p| {
        p["id"] == group["id"] || children.contains(&p["id"])
    }) {
        assert_eq!(profile["chain_id"],dir["chain_id"]);
        let cfg = &profile["settings"];
        assert_eq!(cfg["dex.spectra_registry"], dir["registry"]);
        let key = match cfg["dex.kind"].as_str().unwrap() {
            "spectra_crypto_v1" => "dex.curve_crypto_v1_templates",
            "spectra_stableswap_ng" => "dex.curve_ng_templates",
            kind => panic!("unexpected Spectra pool family {kind}"),
        };
        for m in dir["markets"].as_array().unwrap().iter().filter(|m| m["factory"] == cfg["dex.factory"]) {
            assert!(pools.insert(m["pool"].as_str().unwrap()));
            assert_eq!(m["registered"],true);
            assert_eq!(m["factory_coins"][0],m["ibt"]);
            assert_eq!(m["coin0"],m["ibt"]);
            assert_eq!(m["factory_coins"][1],m["coin1"]);
            assert_ne!(m["coin1"], m["underlying"]);
            assert_ne!(m["coin0"], m["underlying"]);
            let a = m["coin0"].as_str().unwrap();
            let b = m["coin1"].as_str().unwrap();
            let pair = (m["factory"].as_str().unwrap(),a.min(b),a.max(b));
            let count = pairs.entry(pair).or_insert(0u64);
            *count += 1;
            assert!(*count <= cfg["dex.pair_lookup_max"].as_u64().unwrap());
            let template = m["template"].as_str().unwrap();
            assert!(cfg[key].as_array().unwrap().contains(&m["template"]));
            let verified = source["implementations"].as_array().unwrap().iter()
                .find(|i| i["template"] == template).unwrap();
            if verified["kind"] == "legacy" {
                assert_eq!(verified["source_address"], template);
                assert_eq!(verified["runtime_hash"], source["contracts"][template]["keccak256"]);
            } else {
                // У StableSwap NG шаблон содержит код создания. Исходник сверяли по развернутому пулу.
                let sample = dir["markets"].as_array().unwrap().iter()
                    .find(|row| row["pool"] == verified["source_address"]).unwrap();
                assert_eq!(sample["template"], template);
                assert_eq!(sample["factory"], m["factory"]);
                assert!(source["contracts"][template]["bytes"].as_u64().unwrap() > 0);
            }
            templates.insert(template);
        }
    }
    assert_eq!((pools.len(),pairs.len(),templates.len()),(88,88,4));
    assert_eq!(pools.len(),dir["markets"].as_array().unwrap().len());
}
