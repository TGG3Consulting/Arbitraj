//! Сверяет справочник двух сетей с исходными строками каталога и ответами фабрик.
use serde_json::Value;
use std::collections::BTreeSet;

#[test]
fn coverage_preserves_each_catalog_row_and_distinguishes_missing_from_zero_volume() {
    let document = include_str!("../docs/BASE_ETHEREUM_SETTINGS.md");
    let snapshot: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/catalog_recheck.json"
    ))
    .unwrap();
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
        let mut spot = 0;
        let mut provider = 0;
        let mut with_profile = 0;
        let profiles: Vec<Value> =
            serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
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
            // Статус шага 4 стоит перед исходной классификацией сбора; счётчики ниже — по ней.
            let class = source_class(decision);
            if class.starts_with("спот") {
                spot += 1;
                provider += usize::from(class.starts_with("спот через провайдера"));
                if let Some(suffix) = class.strip_prefix("спот · профиль `") {
                    let profile = suffix.split_once('`').unwrap().0;
                    assert!(profiles
                        .iter()
                        .any(|p| p["network"] == key && p["id"] == profile));
                    with_profile += 1;
                } else if key == "base" && id == "1700" {
                    // Остановка работ по Wombat не отменяет прежнюю запись этой сети.
                    let physical: Value = serde_json::from_str(include_str!(
                        "data/first_run_dex_2026-11-26/wombat_base.json"
                    ))
                    .unwrap();
                    let profile = profiles.iter().find(|p| {
                        p["network"] == key
                            && p["chain_id"] == physical["chain_id"]
                            && p["source"] == physical["url"]
                            && p["settings"]["dex.kind"] == "wombat"
                    });
                    let pools = profile.unwrap()["settings"]["dex.stable_pools"]
                        .as_array()
                        .unwrap();
                    assert!(pools.iter().any(|pool| {
                        let addr = pool.as_str().unwrap();
                        physical["documented"].get(addr).is_some()
                            && row.to_ascii_lowercase().contains(addr)
                    }));
                    with_profile += 1;
                }
            }
            if matches!(
                (key, id),
                ("base", "2589" | "622" | "1447" | "7508" | "5458" | "1700")
                    | ("ethereum", "4703" | "1700")
            ) {
                assert_eq!(decision, "спот · оборот < 5 000 USD, отложено");
                assert!(volumes[id].as_f64().is_some_and(|v| (0.0..5000.0).contains(&v)));
            }
            if decision.starts_with("нет спота") {
                nonspot += 1;
                assert!(
                    decision.contains("https://") && decision.contains("#L"),
                    "{key} {id}"
                );
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
                assert_eq!(volume.parse::<f64>().unwrap(), number, "{key} {id}");
                if row.contains("оборот < 5 000 USD, отложено") {
                    assert!(number < 5_000.0, "{key} {id}: {number}");
                }
            } else {
                assert_eq!(volume, "нет данных", "{key} {id}");
                assert!(!decision.contains("оборот < 5 000 USD"), "{key} {id}");
            }
        }
        assert_eq!((ids.len(), known), (catalog_rows, expected_known));
        let summary = format!(
            "Из {catalog_rows} строк оборота спотовый обмен установлен в {spot}, в том числе {provider} через провайдера. В {with_profile} строках"
        );
        assert!(table.contains(&summary), "{key}: {summary}");
        assert!(table.contains(&format!("Для {nonspot} указан статус")));
    }
}

/// Отделяет статус шага 4 (решение 7) от исходной классификации строки сбора.
/// Принимает последнюю ячейку строки, возвращает классификацию без статуса; ошибок нет.
fn source_class(decision: &str) -> String {
    if let Some(rest) = decision.strip_prefix("подключён · профиль `") {
        return format!("спот · профиль `{rest}");
    }
    if let Some(rest) = decision.strip_prefix("оборот < 5 000 USD, отложено · ") {
        return rest.to_string();
    }
    if let Some(rest) = decision.strip_prefix("данных об обороте за сутки нет — отложено · ") {
        return rest.split_once(") · ").map_or(rest, |(_, class)| class).to_string();
    }
    decision.to_string()
}

#[test]
fn catalog_factory_sample_counts_only_confirmed_pairs_and_matching_profile_domains() {
    let bytes = include_bytes!("data/first_run_dex_2026-11-26/catalog_factory_probes.json.gz");
    let archive: Value = serde_json::from_reader(flate2::read::GzDecoder::new(&bytes[..])).unwrap();
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let document = include_str!("../docs/BASE_ETHEREUM_SETTINGS.md");
    // Оборот карты Cypher и плечевых позиций USDN учитывается отдельно от спотовых AMM.
    for (network, id) in [
        ("base", "smardex-base"),
        ("ethereum", "smardex-ethereum"),
        ("ethereum", "cypher-v2-ethereum"),
        ("ethereum", "cypher-v4-ethereum"),
    ] {
        assert!(profiles
            .iter()
            .any(|p| p["network"] == network && p["id"] == id));
    }
    for (net, count, matched_count) in [("base", 64, 64), ("ethereum", 28, 27)] {
        let mut total = 0;
        let mut matching = 0;
        for (id, row) in archive[net]["rows"].as_object().unwrap() {
            if row["factory_roundtrip"].as_str().is_none()
                || row["token0"] == row["token1"]
                || row["token0"].as_str().is_none()
                || row["token1"].as_str().is_none()
            {
                continue;
            }
            total += 1;
            let profile = profiles
                .iter()
                .find(|p| {
                    p["network"] == net
                        && p["settings"]["dex.indexer_id"]
                            .as_str()
                            .or(p["id"].as_str())
                            == Some(id.as_str())
                })
                .unwrap();
            let expected = profile["settings"]["dex.factory"].as_str().unwrap();
            let actual = row["reported_factory"].as_str().unwrap();
            if actual.eq_ignore_ascii_case(expected) {
                matching += 1;
            } else {
                let line = document
                    .lines()
                    .find(|line| line.starts_with(&format!("| `{id}` |")) && line.contains(actual))
                    .unwrap();
                assert!(line.contains(expected) && line.contains("отличается от dex.factory"));
            }
        }
        assert_eq!((total, matching), (count, matched_count));
        assert!(document.contains(archive[net]["source_sha256"].as_str().unwrap()));
    }
}

#[path = "base_ethereum_catalog_coverage/decision_seven.rs"]
mod decision_seven;
