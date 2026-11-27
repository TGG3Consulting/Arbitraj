//! Справочник развёртываний Base и Ethereum для экрана «Первый запуск».
//! Значения идут в форму через first_run::table. Суффикс области тот же, что в адаптере DEX.
//! Источники и версии контрактов хранятся с данными в config/first_run_dex.json.
use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

use super::dex_key::dex_ids;
use super::first_run::{Prefill, Rec, Section};
use super::state::{ApiError, AppState};
use crate::settings::{dex_key::venue_suffix, read::At};

#[derive(Deserialize)]
struct Profile {
    chain_id: u64,
    network: String,
    id: String,
    source: String,
    settings: BTreeMap<String, Parameter>,
    #[serde(default)]
    venue_settings: BTreeMap<String, Parameter>,
}

/// Строковое поле, перечень адресов или сетевые ключи дочерних фабрик (dex.sub_dex).
#[derive(Deserialize)]
#[cfg_attr(test, derive(Debug, PartialEq))]
#[serde(untagged)]
enum Parameter {
    Text(String),
    List(Vec<String>),
    Bool(bool),
    Integer(i64),
}

impl Parameter {
    /// Формат редактора списков — элементы через запятую. Для дочернего ключа применяется
    /// тот же суффикс площадки, что и к родительскому ID (settings/dex_key.rs).
    fn display(&self, name: &str, suffix: &str) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Bool(flag) => flag.to_string(),
            Self::Integer(number) => number.to_string(),
            Self::List(items) if name == "dex.sub_dex" => items
                .iter()
                .map(|id| format!("{id}{suffix}"))
                .collect::<Vec<_>>()
                .join(", "),
            Self::List(items) => items.join(", "),
        }
    }
}

/// Загружает закреплённый справочник один раз. Ошибка означает порчу данных сборки.
fn profiles() -> Result<&'static [Profile], ApiError> {
    static PROFILES: OnceLock<Result<Vec<Profile>, String>> = OnceLock::new();
    PROFILES
        .get_or_init(|| {
            serde_json::from_str(include_str!("../../config/first_run_dex.json"))
                .map_err(|e| e.to_string())
        })
        .as_deref()
        .map_err(ApiError::internal)
}

/// Подсказки видны и до регистрации сети. Предзаполнение доступно после однозначного выбора её области.
/// Ошибка — только при повреждении встроенного справочника. Настройки не записываются.
pub async fn sections(st: &AppState) -> Result<Vec<Section>, ApiError> {
    let ids = dex_ids(st).await;
    let mut scopes = BTreeMap::new();
    for id in &ids {
        let at = At {
            venue: Some(id),
            coin: None,
        };
        let (Ok(chain_id), Ok(network), Ok(suffix)) = (
            st.settings.integer("dex.evm_chain_id", at),
            st.settings.text("dex.chain", at),
            venue_suffix(&st.settings, id, ids.len() == 1),
        ) else {
            continue;
        };
        scopes.insert(
            (chain_id, network),
            (suffix.unwrap_or_default(), id.clone()),
        );
    }
    let mut sections = Vec::new();
    for p in profiles()? {
        let place = scopes.get(&(p.chain_id, p.network.clone()));
        let suffix = place.map(|(s, _)| s);
        let mut rec: Vec<_> = p
            .settings
            .iter()
            .map(|(name, value)| Rec {
                name: name.clone(),
                area: format!("{} · {}", p.network, p.id),
                unit: None,
                value: Some(value.display(name, suffix.map(String::as_str).unwrap_or(""))),
                method: None,
                source: Some(p.source.clone()),
                rationale: format!("Параметр размещения {} в сети {}.", p.id, p.chain_id),
                prefill: suffix.map(|s| Prefill {
                    kind: "Dex".into(),
                    id: Some(format!("{}{s}", p.id)),
                }),
            })
            .collect();
        rec.extend(p.venue_settings.iter().map(|(name, value)| Rec {
            name: name.clone(),
            area: p.network.clone(),
            unit: None,
            value: Some(value.display(name, "")),
            method: None,
            source: Some(p.source.clone()),
            rationale: format!("Сетевое чтение контрактов {}.", p.id),
            prefill: place.map(|(_, venue)| Prefill {
                kind: "Venue".into(),
                id: Some(venue.clone()),
            }),
        }));
        sections.push(Section {
            title: format!("{} · {}", p.network, p.id),
            intro: format!("Сеть {}. Результаты проверки пулов и квотеров приведены в docs/BASE_ETHEREUM_SETTINGS.md.", p.chain_id),
            rec,
        });
    }
    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{catalog, scope::Scope, value::validate};
    use serde_json::Value;

    /// Каждое предложенное значение принимает обычный редактор. Каждому адресу соответствует
    /// непустой runtime на том же chainId и контрольном блоке.
    #[test]
    fn deferred_markets_are_absent_from_the_first_run_network() {
        let rows = profiles().unwrap_or_else(|e| panic!("{}", e.1));
        for (chain, id) in [
            (1, "basin-wells-ethereum"),
            (8453, "pinto-exchange-base"),
            (1, "wombat-exchange"),
            (1, "wombat-volatile-ethereum"),
        ] {
            assert!(!rows.iter().any(|p| p.chain_id == chain && p.id == id), "{chain}/{id}");
        }
        assert!(rows.iter().any(|p| p.chain_id == 8453 && p.id == "wombat-exchange"));
    }

    #[test]
    fn deployments_match_settings_schema_and_recorded_code() {
        let base: Value = serde_json::from_str(include_str!(
            "../../tests/data/first_run_dex_2026-11-26/base.json"
        ))
        .unwrap();
        let eth: Value = serde_json::from_str(include_str!(
            "../../tests/data/first_run_dex_2026-11-26/ethereum.json"
        ))
        .unwrap();
        let all = profiles().unwrap_or_else(|_| panic!("профили DEX повреждены"));
        let mut pairs: Vec<Value> = serde_json::from_str(include_str!(
            "../../tests/data/base_ethereum_v2_probes.json"
        ))
        .unwrap();
        pairs.extend(
            serde_json::from_str::<Vec<Value>>(include_str!(
                "../../tests/data/base_ethereum_v2_variable_fees.json"
            ))
            .unwrap(),
        );
        for (network, bytes) in [
            ("base", include_bytes!("../../tests/data/first_run_dex_2026-11-26/docswap_base_simulated.json.gz").as_slice()),
            ("ethereum", include_bytes!("../../tests/data/first_run_dex_2026-11-26/docswap_ethereum_simulated.json.gz").as_slice()),
        ] {
            let data: Value = serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap();
            assert_eq!(data["original_count"], 0);
            assert_eq!(data["cases"].as_array().unwrap().len(), 8);
            pairs.push(serde_json::json!({"network":network,"dex":format!("dex-on-crypto-{network}"),
                "creation_code_hash":data["init_hash"]}));
        }
        for (network, bytes, parent) in [
            ("base", include_bytes!("../../tests/data/first_run_dex_2026-11-26/sectorone_v2_base.json.gz").as_slice(), &base),
            ("ethereum", include_bytes!("../../tests/data/first_run_dex_2026-11-26/sectorone_v2_ethereum.json.gz").as_slice(), &eth),
        ] {
            let recorded: Value = serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap();
            let id = format!("sectorone-v2-{network}");
            assert_eq!(recorded["selector"], parent["selector"]);
            let changed = &recorded["changed_fee"];
            assert_eq!(recorded["pair"], changed["pair"]);
            assert_eq!(recorded["original_count"], 0);
            assert_eq!((recorded["fee"].as_u64(), changed["fee"].as_u64()), (Some(300), Some(400)));
            for (old, new) in recorded["cases"].as_array().unwrap().iter().zip(changed["cases"].as_array().unwrap()) {
                for key in ["mode", "source", "target", "input"] {
                    assert_eq!(old[key], new[key]);
                }
                assert!(old["output"].as_u64().unwrap() > new["output"].as_u64().unwrap());
            }
            assert_eq!(recorded["cases"].as_array().unwrap().len(), 8);
            assert_eq!(changed["cases"].as_array().unwrap().len(), 8);
            pairs.push(serde_json::json!({"network":network,"dex":id,"creation_code_hash":recorded["init_hash"]}));
        }
        let moto: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
            include_bytes!("../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum.json.gz").as_slice(),
        )).unwrap();
        assert_eq!(moto["selector"], eth["selector"]);
        let directory = &moto["directory"];
        let factory = directory["roles"]["factory"].as_str().unwrap().parse().unwrap();
        let init = directory["init"].as_str().unwrap().parse().unwrap();
        let profile = all.iter().find(|p| p.id == "motoswap-ethereum").unwrap();
        let shells = directory["pairs"].as_array().unwrap();
        assert_eq!(shells.len(), 22);
        for pair in shells {
            let address = |v: &Value| v.as_str().unwrap().parse::<alloy_primitives::Address>().unwrap();
            let tokens = &pair["tokens"];
            let predicted = crate::venues::bsc_dex::pools::verify::v2_create2(
                factory, address(&tokens[0]), address(&tokens[1]), init,
            );
            assert_eq!(predicted, address(&pair["pool"]));
            assert_eq!(profile.settings.get("dex.pair_runtime_hash"),
                Some(&Parameter::Text(pair["runtime"].as_str().unwrap().into())));
        }
        pairs.push(serde_json::json!({"network":"ethereum","dex":"motoswap-ethereum","creation_code_hash":directory["init"]}));
        let theorem: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
            include_bytes!("../../tests/data/first_run_dex_2026-11-26/theorem_ethereum.json.gz").as_slice(),
        )).unwrap();
        assert_eq!(theorem["directory"]["selector"], eth["selector"]);
        pairs.push(serde_json::json!({"network":"ethereum","dex":"theorem-ethereum","creation_code_hash":theorem["directory"]["init"]}));
        let goblins: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
            include_bytes!("../../tests/data/first_run_dex_2026-11-26/goblins_base_wallets.json.gz").as_slice(),
        )).unwrap();
        assert_eq!(goblins["selector"], base["selector"]);
        pairs.push(serde_json::json!({"network":"base","dex":"goblins-base","creation_code_hash":goblins["init"]}));
        let aquila: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
            include_bytes!("../../tests/data/first_run_dex_2026-11-26/rubicon_aquila_ethereum.json.gz").as_slice(),
        )).unwrap();
        assert_eq!(aquila["selector"], eth["selector"]);
        assert_eq!(aquila["live_rows"][0]["reserves"], serde_json::json!(["0","0"]));
        assert_eq!(aquila["cases"].as_array().unwrap().len(), 8);
        pairs.push(serde_json::json!({"network":"ethereum","dex":"rubicon-aquila-ethereum","creation_code_hash":aquila["init_hash"]}));
        let base_mars: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
            include_bytes!("../../tests/data/first_run_dex_2026-11-26/marswap_base_simulated.json.gz").as_slice(),
        )).unwrap();
        assert_eq!(base_mars["selector"], base["selector"]);
        assert_eq!(base_mars["original_count"], 0);
        assert_eq!(base_mars["cases"].as_array().unwrap().len(), 12);
        pairs.push(serde_json::json!({"network":"base","dex":"marswap-base","creation_code_hash":base_mars["init_hash"]}));
        let omni: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
            include_bytes!("../../tests/data/first_run_dex_2026-11-26/omni-v2-base.json.gz").as_slice(),
        )).unwrap();
        assert_eq!(omni["selector"], base["selector"]);
        pairs.extend(omni["examples"].as_array().unwrap().iter().filter_map(|v| v.get("measured")).cloned());
        let mars: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
            include_bytes!("../../tests/data/first_run_dex_2026-11-26/marswap-ethereum.json.gz").as_slice(),
        )).unwrap();
        assert_eq!(mars["selector"], eth["selector"]);
        assert_eq!(mars["complete_round_trips"], 12);
        pairs.push(serde_json::json!({"network":"ethereum","dex":mars["id"],"creation_code_hash":mars["init_hash"]}));
        let markets = [
            include_str!("../../tests/data/first_run_dex_2026-11-26/ethereum_v2_factories.json"),
            include_str!("../../tests/data/first_run_dex_2026-11-26/base_v2_factories.json"),
        ];
        let blue: Value = serde_json::from_str(include_str!(
            "../../tests/data/first_run_dex_2026-11-26/bluedex_base.json"
        )).unwrap();
        pairs.extend(blue["samples"].as_array().unwrap().iter().filter(|row|row["cases"].is_array()).cloned());
        let white: Value = serde_json::from_str(include_str!(
            "../../tests/data/first_run_dex_2026-11-26/whiteswap_directory.json"
        ))
        .unwrap();
        pairs.extend(
            white["samples"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|s| s["cases"].is_array())
                .cloned(),
        );
        for (network, snapshot, raw) in [
            ("base", &base, include_bytes!("../../tests/data/first_run_dex_2026-11-26/llama_base_v2_wallets.json.gz").as_slice()),
            ("ethereum", &eth, include_bytes!("../../tests/data/first_run_dex_2026-11-26/llama_eth_v2_wallets.json.gz").as_slice()),
        ] {
            let records: std::collections::BTreeMap<String,Value> =
                serde_json::from_reader(flate2::read::GzDecoder::new(raw)).unwrap();
            for (slug, record) in records {
                assert_eq!(record["selector"], snapshot["selector"]);
                let sample = record["samples"].as_array().unwrap().iter()
                    .find(|s| s["cases"].as_array().is_some_and(|v| v.len() == 2)).unwrap();
                assert_eq!(sample["network"],network);
                assert_eq!(sample["dex"],format!("{slug}-{network}"));
                assert_eq!(sample["factory"],record["factory"]);
                assert_eq!(sample["creation_code_hash"],record["origin"]["creation_code_hash"]);
                pairs.push(sample.clone());
            }
        }
        let shiba_bytes=include_bytes!("../../tests/data/first_run_dex_2026-11-26/shibaswap-v2_ethereum_directory.json.gz");
        let shiba: Value = serde_json::from_reader(flate2::read::GzDecoder::new(shiba_bytes.as_slice())).unwrap();
        assert_eq!(shiba["selector"],eth["selector"]);
        assert_eq!(shiba["markets"].as_array().unwrap().len(),110);
        pairs.push(serde_json::json!({"dex":"shibaswap-v2-ethereum","network":"ethereum","creation_code_hash":shiba["init"]}));
        let cl_bytes=include_bytes!("../../tests/data/first_run_dex_2026-11-26/wagmi_thick_base.json.gz");
        let cl_books: std::collections::BTreeMap<String,Value> =
            serde_json::from_reader(flate2::read::GzDecoder::new(cl_bytes.as_slice())).unwrap();
        for (slug, book) in cl_books {
            assert_eq!(book["selector"],base["selector"]);
            assert!(book["cases"].as_array().unwrap().iter().any(|c| c["wire"]=="v5"));
            pairs.push(serde_json::json!({"dex":format!("{slug}-base"), "network":"base", "creation_code_hash":book["init"]}));
        }
        for raw in markets {
            let registered: std::collections::BTreeMap<String, Value> =
                serde_json::from_str(raw).unwrap();
            for (id, record) in &registered {
                if let Some(native) = record["native_router"]["samples"].as_array()
                    && native
                        .iter()
                        .any(|s| s["cases"].as_array().is_some_and(|cases| cases.len() == 2))
                {
                    pairs.push(serde_json::json!({"dex":id, "network":"base", "creation_code_hash":record["origin"]["creation_code_hash"]}));
                }
                pairs.extend(
                    record["samples"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|s| s["cases"].is_array())
                        .cloned(),
                );
            }
        }
        let candy:Value=serde_json::from_str(include_str!("../../tests/data/first_run_dex_2026-11-26/candyswap_base.json")).unwrap();
        let diamond:Value=serde_json::from_str(include_str!("../../tests/data/first_run_dex_2026-11-26/diamondswap_base.json")).unwrap();
        for (id, hash, selector) in [
            ("candyswap",&candy["origin"]["creation_code_hash"],&candy["selector"]),
            ("diamondswap",&diamond["creation"]["hash"],&diamond["selector"]),
        ] {
            assert_eq!(selector,&base["selector"]);
            pairs.push(serde_json::json!({"dex":id,"network":"base","creation_code_hash":hash}));
        }
        let raw=include_bytes!("../../tests/data/first_run_dex_2026-11-26/sashimi_ethereum_registry.json.gz");
        let sashimi:Value=serde_json::from_reader(flate2::read::GzDecoder::new(raw.as_slice())).unwrap();
        assert_eq!(sashimi["selector"],eth["selector"]);
        pairs.push(serde_json::json!({"dex":"sashimiswap","network":"ethereum","creation_code_hash":sashimi["origin"]["creation_code_hash"]}));
        let bitswap:Value=serde_json::from_str(include_str!("../../tests/data/first_run_dex_2026-11-26/bitswap_ethereum_registry.json")).unwrap();
        assert_eq!(bitswap["block_hash"],eth["selector"]["blockHash"]);
        assert_eq!(bitswap["chain"],eth["chain_id"]);
        pairs.push(serde_json::json!({"dex":"bitswap","network":"ethereum","creation_code_hash":bitswap["init"]}));
        for p in all {
            let snapshot = [&base, &eth]
                .into_iter()
                .find(|s| s["chain_id"] == p.chain_id)
                .unwrap();
            if p.id == "yield-basis-ethereum" {
                let market: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
                    include_bytes!("../../tests/data/first_run_dex_2026-11-26/yield_basis_ethereum.json.gz").as_slice())).unwrap();
                let address=market["factory"].as_str().unwrap();
                assert_eq!(market["selector"],snapshot["selector"]);
                assert_eq!(market["factory_code"],snapshot["contracts"][address]);
                assert_eq!(p.settings.get("dex.factory"),Some(&Parameter::Text(address.into())));
                assert_eq!(p.settings.get("dex.factory_runtime_hash"),Some(&Parameter::Text(market["factory_code"]["keccak256"].as_str().unwrap().into())));
            }
            if p.id == "balancer-v1-ethereum" {
                let original: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
                    include_bytes!("../../tests/data/first_run_dex_2026-11-26/balancer_v1_ethereum.json.gz").as_slice())).unwrap();
                let wallet: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
                    include_bytes!("../../tests/data/first_run_dex_2026-11-26/balancer_v1_wallet.json.gz").as_slice())).unwrap();
                let tokens: Value = serde_json::from_str(include_str!(
                    "../../tests/data/first_run_dex_2026-11-26/ethereum_exact_transfers.json")).unwrap();
                for source in [&original, &wallet, &tokens] { assert_eq!(source["selector"],snapshot["selector"]); }
                for (key, value) in [("dex.factory",&original["factory"]),("dex.factory_runtime_hash",&original["factory_code"]["keccak256"])] {
                    assert_eq!(p.settings.get(key),Some(&Parameter::Text(value.as_str().unwrap().into())));
                }
                let pools: Vec<_> = original["markets"].as_object().unwrap().values().filter_map(|v|v.get("code")).collect();
                assert!(!pools.is_empty());
                for pool in pools {
                    assert_eq!(p.settings.get("dex.pair_runtime_hash"),Some(&Parameter::Text(pool["keccak256"].as_str().unwrap().into())));
                }
                assert_eq!(p.settings.get("dex.module_runtime_hashes"), Some(&Parameter::List(vec![wallet["module_hash"].as_str().unwrap().into()])));
                let mut known: Vec<_> = tokens["tokens"].as_object().unwrap().values().map(|t|
                    format!("{} {}",t["address"].as_str().unwrap(),t["keccak256"].as_str().unwrap())).collect();
                let Some(Parameter::List(actual)) = p.settings.get("dex.router_exact_tokens") else { panic!("missing BPool exact-transfer bindings") };
                let mut actual = actual.clone();actual.sort();known.sort();assert_eq!(actual,known);
            }
            if p.id == "bancor-v3-ethereum" {
                let markets: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
                    include_bytes!("../../tests/data/first_run_dex_2026-11-26/bancor_v3_ethereum.json.gz").as_slice(),
                )).unwrap();
                let wallet: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
                    include_bytes!("../../tests/data/first_run_dex_2026-11-26/bancor_v3_wallet.json.gz").as_slice(),
                )).unwrap();
                let tokens: Value = serde_json::from_str(include_str!(
                    "../../tests/data/first_run_dex_2026-11-26/bancor_v3_exact_tokens.json"
                )).unwrap();
                for archive in [&markets, &wallet, &tokens] {
                    assert_eq!(archive["selector"], snapshot["selector"]);
                }
                let text = |v: &Value| v.as_str().unwrap().to_owned();
                let list = |key: &str| {
                    let Some(Parameter::List(items)) = p.settings.get(key) else { panic!("missing {key}") };
                    let mut rows = items.clone();
                    rows.sort();
                    rows
                };
                let roles = &markets["roles"];
                for (key, role) in [("dex.factory", "BancorNetwork"), ("dex.quoter", "BancorNetworkInfo"), ("dex.vault", "MasterVault")] {
                    assert_eq!(p.settings.get(key), Some(&Parameter::Text(text(&roles[role]["address"]))));
                }
                let mut bindings: Vec<_> = roles.as_object().unwrap().values().map(|r| {
                    let proxy = &r["implementation"];
                    assert_eq!(proxy["address"], r["manifest_implementation"]);
                    [text(&r["address"]), text(&r["keccak256"]), text(&proxy["address"]), text(&proxy["keccak256"])].join(" ")
                }).collect();
                bindings.sort();
                assert_eq!(list("dex.bancor_roles"), bindings);
                let mut collections: Vec<_> = markets["collections"].as_object().unwrap().iter()
                    .map(|(address, c)| format!("{address} {}", text(&c["keccak256"]))).collect();
                collections.sort();
                assert_eq!(list("dex.bancor_collections"), collections);
                let mut assets: Vec<_> = tokens["tokens"].as_object().unwrap().iter()
                    .map(|(address, t)| format!("{address} {}", text(&t["keccak256"]))).collect();
                assets.sort();
                assert_eq!(list("dex.router_exact_tokens"), assets);
                assert_eq!(list("dex.module_runtime_hashes"), vec![text(&wallet["module_hash"])]);
            }
            if p.id=="uniswap-v1-ethereum"{
                let bytes=include_bytes!("../../tests/data/first_run_dex_2026-11-26/uniswap_v1_ethereum.json.gz");
                let evidence:Value=serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
                assert_eq!(evidence["selector"],snapshot["selector"]);
                for (setting,role) in [("dex.factory_runtime_hash","factory"),("dex.v1_template_runtime_hash","template")]{
                    assert_eq!(p.settings.get(setting),Some(&Parameter::Text(evidence[role]["runtime"].as_str().unwrap().into())));
                }
                for m in evidence["markets"].as_object().unwrap().values(){
                    assert_eq!(p.settings.get("dex.pair_runtime_hash"),Some(&Parameter::Text(m["runtime"].as_str().unwrap().into())));
                }
                assert_eq!(p.settings.get("dex.module_runtime_hashes"),Some(&Parameter::List(vec![evidence["module_hash"].as_str().unwrap().into()])));
            }
            let scope = Scope::Dex(format!("{}@{}", p.id, p.network));
            for (k, v) in &p.settings {
                let def = catalog::find(k).expect("настройка профиля должна быть в схеме");
                let values = match v {
                    Parameter::Text(text) => vec![text],
                    Parameter::List(items) => items.iter().collect(),
                    Parameter::Bool(_) | Parameter::Integer(_) => Vec::new(),
                };
                let input = match v {
                    Parameter::Text(t) => Value::String(t.clone()),
                    Parameter::List(xs) => serde_json::json!(xs),
                    Parameter::Bool(flag) => Value::Bool(*flag),
                    Parameter::Integer(number) => Value::from(*number),
                };
                validate(def, &scope, &input).unwrap_or_else(|e| panic!("{} {k}: {e}", p.id));
                for text in values {
                    // Нулевой адрес означает выключенный модуль у конкретной фабрики или крючка этой сети.
                    if k == "dex.aero_mev_module"
                        && text == "0x0000000000000000000000000000000000000000"
                    {
                        let (raw, setting) = if matches!(p.settings.get("dex.kind"), Some(Parameter::Text(s)) if s == "aero_basic")
                        {
                            (
                                include_str!(
                                    "../../tests/data/first_run_dex_2026-11-26/aero_basic_base.json"
                                ),
                                "dex.factory",
                            )
                        } else {
                            (
                                include_str!(
                                    "../../tests/data/first_run_dex_2026-11-26/metadex_ethereum.json"
                                ),
                                "dex.cl_swap_hook",
                            )
                        };
                        let actual: Value = serde_json::from_str(raw).unwrap();
                        let Some(Parameter::Text(target)) = p.settings.get(setting) else {
                            panic!("{} отсутствует {setting}", p.id);
                        };
                        assert_eq!(actual["chain_id"], p.chain_id);
                        assert_eq!(actual["selector"], snapshot["selector"]);
                        assert!(actual["reads"].as_array().unwrap().iter().any(|r| {
                            r["to"]
                                .as_str()
                                .is_some_and(|a| a.eq_ignore_ascii_case(target))
                                && r["signature"] == "mevTaxModule()"
                                && r["response"]
                                    .as_str()
                                    .is_some_and(|v| v == format!("0x{}", "0".repeat(64)))
                        }));
                        continue;
                    }
                    if k == "dex.bancor_native_token" && p.id == "bancor-v21-ethereum" {
                        let bytes = include_bytes!("../../tests/data/first_run_dex_2026-11-26/bancor_v21_wallet.json.gz");
                        let proof: Value = serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
                        assert_eq!(proof["manifest"]["sourceBlock"], snapshot["selector"]["blockHash"]);
                        let cases = proof["controls"].as_array().unwrap();
                        for input in [true, false] {
                            assert!(cases.iter().any(|row| {
                                let path = row["path"].as_array().unwrap();
                                let endpoint = if input {path.first()} else {path.last()};
                                row["mode"] == "native_wallet" && endpoint.and_then(Value::as_str) == Some(text.as_str()) &&
                                    row["spent"].as_str().is_some_and(|n| n != "0") &&
                                    row["received"].as_str().is_some_and(|n| n != "0")
                            }));
                        }
                        continue;
                    }
                    if k=="dex.bancor_native_token" && p.id=="bancor-v3-ethereum"{
                        let bytes=include_bytes!("../../tests/data/first_run_dex_2026-11-26/bancor_v3_wallet.json.gz");
                        let proof:Value=serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
                        assert_eq!(proof["selector"],snapshot["selector"]);
                        assert!(proof["cases"].as_array().unwrap().iter().any(|v|v["source"]==text.as_str() && v["mode"]=="native_wallet" && v["spent"].is_string()));
                        continue;
                    }
                    if text.len() == 42 && text.starts_with("0x") {
                        assert!(
                            snapshot["contracts"][text.to_ascii_lowercase()]["bytes"]
                                .as_u64()
                                .is_some_and(|size| size > 0),
                            "{} / {} / {k}: нет записи о коде адреса {text}",
                            p.network,
                            p.id
                        );
                    } else if k=="dex.pool_code_hashes" && p.id=="sharkswap-base" {
                        let data=include_bytes!("../../tests/data/first_run_dex_2026-11-26/sharkswap_base_wallets.json.gz");
                        let archive:Value=serde_json::from_reader(flate2::read::GzDecoder::new(&data[..])).unwrap();
                        assert_eq!(archive["selector"],snapshot["selector"]);
                        assert_eq!(archive["init"],text.as_str());
                        assert!(matches!(p.settings.get("dex.factory"),Some(Parameter::Text(a)) if archive["roles"]["factory"]["address"]==a.as_str()));
                    } else if k=="dex.pool_code_hashes" && matches!(p.id.as_str(),"cypher-v2-ethereum"|"degenswap-ethereum") {
                        let data:&[u8]=if p.id=="degenswap-ethereum" {include_bytes!("../../tests/data/first_run_dex_2026-11-26/degenswap_ethereum_directory.json.gz")}else{include_bytes!("../../tests/data/first_run_dex_2026-11-26/cypher_v2_ethereum_directory.json.gz")};
                        let d:Value=serde_json::from_reader(flate2::read::GzDecoder::new(data)).unwrap();
                        assert_eq!(d["selector"],snapshot["selector"]);
                        assert_eq!(d["init"]["hash"],text.as_str());
                        assert!(matches!(p.settings.get("dex.factory"),Some(Parameter::Text(a)) if d["factory"]["address"]==a.as_str()));
                    } else if k=="dex.pool_code_hashes" && matches!(p.settings.get("dex.kind"),Some(Parameter::Text(kind)) if kind=="safeswap_trade_module") {
                        let bytes=include_bytes!("../../tests/data/first_run_dex_2026-11-26/safeswap_ethereum_wallets.json.gz");
                        let archive:Value=serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
                        let dep=&archive["deployments"][3];
                        assert_eq!(archive["selector"],snapshot["selector"]);
                        assert_eq!(dep["creation_code"]["hash"],text.as_str());
                        for (key,role) in [("dex.quoter",&dep["access"]["trade"]),("dex.safeswap_spender",&dep["roles"]["router"]),
                                            ("dex.factory",&dep["roles"]["factory"])] {
                            assert!(matches!(p.settings.get(key),Some(Parameter::Text(a)) if role["address"]==a.as_str()));
                        }
                     } else if k=="dex.pool_code_hashes" && matches!(p.settings.get("dex.kind"),Some(Parameter::Text(kind)) if kind=="safeswap_proxy_v5") {
                        let bytes=include_bytes!("../../tests/data/first_run_dex_2026-11-26/safeswap_ethereum_wallets.json.gz");
                        let archive:Value=serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
                        let dep=&archive["deployments"][2];
                        assert_eq!(archive["selector"],snapshot["selector"]);
                        assert_eq!(dep["creation_code"]["hash"],text.as_str());
                        let fee = u32::try_from(dep["pair_pips"].as_u64().unwrap()).unwrap();
                        let h = crate::venues::bsc_dex::pools::safeswap::fee_rule(1,dep["roles"]["factory"]["address"].as_str().unwrap().parse().unwrap(),
                            text.parse().unwrap(),dep["markets"][0]["runtime"].as_str().unwrap().parse().unwrap(),fee);
                        assert!(matches!(p.settings.get("dex.v2_fee_rule_hash"),Some(Parameter::Text(v)) if v==&format!("{h:#x}")));
                        assert!(matches!(p.settings.get("dex.factory"),Some(Parameter::Text(a)) if a==dep["roles"]["factory"]["address"].as_str().unwrap()));
                        assert!(matches!(p.settings.get("dex.pair_runtime_hash"),Some(Parameter::Text(h)) if dep["markets"].as_array().unwrap().iter().all(|m|m["runtime"]==h.as_str())));
                    } else if k=="dex.pool_code_hashes" && matches!(p.settings.get("dex.kind"),Some(Parameter::Text(kind)) if kind=="safeswap_v2") {
                        let bytes=include_bytes!("../../tests/data/first_run_dex_2026-11-26/safeswap_ethereum_directory.json.gz");
                        let dir:Value=serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
                        assert_eq!(dir["selector"],snapshot["selector"]);
                        let address=match p.settings.get("dex.factory").unwrap() {Parameter::Text(a)=>a,_=>unreachable!()};
                        let r=dir["deployments"].as_array().unwrap().iter().find(|r|r["roles"]["factory"]["address"]==address.as_str()).unwrap();
                        assert_eq!(r["init"].as_str(),Some(text.as_str()));
                        assert!(matches!(p.settings.get("dex.v2_router_fee_pips"),Some(Parameter::Integer(pips)) if r["router_pips"].as_u64()==Some(*pips as u64)));
                    } else if k=="dex.pool_code_hashes" && matches!(p.id.as_str(),"blueprint"|"vvs-v3-ethereum") {
                        let raw:&[u8]=if p.id=="blueprint" { include_bytes!("../../tests/data/first_run_dex_2026-11-26/blueprint_ethereum_directory.json.gz") }
                            else { include_bytes!("../../tests/data/first_run_dex_2026-11-26/vvs_ethereum_directory.json.gz") };
                        let r:Value=serde_json::from_reader(flate2::read::GzDecoder::new(raw)).unwrap();
                        assert_eq!(r["selector"],snapshot["selector"]);
                        assert_eq!(r["init"].as_str(),Some(text.as_str()));
                        assert_eq!(r["nonce"].as_u64().unwrap(),r["markets"].as_array().unwrap().len() as u64+1);
                    } else if k=="dex.pool_code_hashes" && p.id=="bundles-swap" {
                        let bytes=include_bytes!("../../tests/data/first_run_dex_2026-11-26/bundles_ethereum_directory.json.gz");
                        let d:Value=serde_json::from_reader(flate2::read::GzDecoder::new(&bytes[..])).unwrap();
                        assert_eq!(d["selector"],snapshot["selector"]);
                        assert!(["Pair","OraclePair"].iter().any(|c|d["classes"][*c]["init"]==text.as_str()));
                    } else if k=="dex.pool_code_hashes" && p.id=="antfarm-ethereum" {
                        let raw=include_bytes!("../../tests/data/first_run_dex_2026-11-26/antfarm_ethereum_registry.json.gz");
                        let r:Value=serde_json::from_reader(flate2::read::GzDecoder::new(&raw[..])).unwrap();
                        assert_eq!(r["selector"],snapshot["selector"]);
                        assert!(r["init_hashes"].as_array().unwrap().iter().any(|v|v.as_str()==text.strip_prefix("0x")));
                    } else if k == "dex.pool_code_hashes" && matches!(p.settings.get("dex.kind"), Some(Parameter::Text(kind)) if kind=="solidly_v2_proxy") {
                        let bytes = include_bytes!("../../tests/data/first_run_dex_2026-11-26/solidly_v2_ethereum.json.gz");
                        let data: Value = serde_json::from_reader(flate2::read::GzDecoder::new(&bytes[..])).unwrap();
                        assert_eq!(data["selector"], snapshot["selector"]);
                        assert_eq!(data["init"].as_str(), Some(text.as_str()));
                    } else if k=="dex.pool_code_hashes" && p.id=="cypher-v4-ethereum"{
                        let d:Value=serde_json::from_reader(flate2::read::GzDecoder::new(include_bytes!("../../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_directory.json.gz").as_slice())).unwrap();
                        assert_eq!(d["init"],text.as_str());assert_eq!(d["selector"],snapshot["selector"]);
                    } else if k == "dex.pool_code_hashes" && matches!(p.settings.get("dex.kind"),Some(Parameter::Text(kind)) if kind=="algebra_integral_supernova") {
                        let data:Value=serde_json::from_str(include_str!("../../tests/data/first_run_dex_2026-11-26/supernova_ethereum_directory.json")).unwrap();
                        assert_eq!(data["selector"],snapshot["selector"]);
                        assert_eq!(data["init"].as_str(),Some(text.as_str()));
                    } else if k == "dex.pool_code_hashes" && matches!(p.settings.get("dex.kind"),Some(Parameter::Text(kind)) if matches!(kind.as_str(),"algebra_integral_base_plugin_v1"|"algebra_integral_treble"|"algebra_integral_treble_spot")) {
                        let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/first_run_dex_2026-11-26");
                        let multi = p.id == "thirdfy";
                        let path=root.join(if multi { "thirdfy_base_wallets.json.gz".to_owned() } else { format!("base_algebra_{}.json.gz",p.id) });
                        let file=std::fs::File::open(&path).unwrap_or_else(|err| panic!("{}: {err}",path.display()));
                        let data:Value=serde_json::from_reader(flate2::read::GzDecoder::new(file)).unwrap();
                        assert_eq!(data["selector"],snapshot["selector"]);
                        let entries = if multi {
                            let all=data["markets"].as_array().unwrap();
                            assert_eq!(all.len(),6);
                            all.as_slice()
                        } else { std::slice::from_ref(&data) };
                        for entry in entries {
                            assert_eq!(entry["selector"],snapshot["selector"]);
                            assert_eq!(entry["reads"]["POOL_INIT_CODE_HASH()"].as_str(),Some(text.as_str()));
                            if multi {
                                assert!(matches!(p.settings.get("dex.factory"),Some(Parameter::Text(a)) if entry["roles"]["factory"]["address"]==a.as_str()));
                                assert!(matches!(p.settings.get("dex.factory_runtime_hash"),Some(Parameter::Text(h)) if entry["roles"]["factory"]["keccak"]==h.as_str()));
                            }
                        }
                    } else if k == "dex.pool_code_hashes"
                        && matches!(p.settings.get("dex.kind"), Some(Parameter::Text(kind)) if (p.chain_id == 8453 && kind == "solidly_v1") || (p.chain_id == 1 && kind == "solidly_v1_generator")) {
                        let network = if p.chain_id == 1 { "ethereum" } else { "base" };
                        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                            .join(format!("tests/data/first_run_dex_2026-11-26/{network}_classic_registry_{}.json.gz",p.id));
                        let input=flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap());
                        let data:Value=serde_json::from_reader(input).unwrap();
                        assert_eq!(data["selector"], snapshot["selector"]);
                        assert!(data["init"].as_str()==Some(text.as_str()) || data["historical"].as_array()
                            .is_some_and(|rows| rows.iter().any(|r| r["init"].as_str()==Some(text.as_str())
                                && r["selector"]["requireCanonical"]==true && r["implementation"]["runtime_hash"].is_string())));
                    } else if k == "dex.pool_code_hashes" && matches!(p.id.as_str(),"ethervista"|"ethervista-base") {
                        let raw=if p.network=="base" {include_str!("../../tests/data/first_run_dex_2026-11-26/ethervista_base.json")}else{include_str!("../../tests/data/first_run_dex_2026-11-26/ethervista_ethereum.json")};
                        let archive:Value=serde_json::from_str(raw).unwrap();
                        assert_eq!(archive["selector"],snapshot["selector"]);
                        assert_eq!(archive["init"].as_str(),Some(text.as_str()));
                    } else if k=="dex.pool_code_hashes" && matches!(p.settings.get("dex.kind"),Some(Parameter::Text(kind)) if kind=="hello_v3") {
                        let raw=if p.network=="base" {include_str!("../../tests/data/first_run_dex_2026-11-26/hello_base_directory.json")} else {include_str!("../../tests/data/first_run_dex_2026-11-26/hello_ethereum_directory.json")};
                        let archive:Value=serde_json::from_str(raw).unwrap();
                        assert_eq!(archive["selector"],snapshot["selector"]);
                        assert_eq!(archive["init_hash"].as_str(),Some(text.as_str()));
                        assert_eq!(archive["nonce"].as_u64().unwrap(),archive["markets"].as_array().unwrap().len() as u64+1);
                    } else if k=="dex.pool_code_hashes" && matches!(p.settings.get("dex.kind"),Some(Parameter::Text(kind)) if kind=="registered_v3") {
                        let evidence: Value=serde_json::from_str(include_str!("../../tests/data/first_run_dex_2026-11-26/registered_v3_base_profiles.json")).unwrap();
                        assert_eq!(evidence["selector"],snapshot["selector"]);
                        assert_eq!(evidence["markets"][p.id.as_str()]["init"].as_str(),Some(text.as_str()));
                    } else if k == "dex.pool_code_hashes" && matches!(p.id.as_str(), "wagmi-ethereum" | "lif3-ethereum") {
                        let registry=if p.id=="lif3-ethereum" {include_str!("../../tests/data/first_run_dex_2026-11-26/lif3_ethereum_registry.json")}
                            else {include_str!("../../tests/data/first_run_dex_2026-11-26/wagmi_ethereum_registry.json")};
                        let evidence: Value=serde_json::from_str(registry).unwrap();
                        assert_eq!(evidence["selector"],snapshot["selector"]);
                        assert_eq!(evidence["init"].as_str(),Some(text.as_str()));
                    } else if k=="dex.pool_code_hashes" && p.id=="vectaswap-base" {
                        let proof:Value=serde_json::from_slice(include_bytes!("../../tests/data/first_run_dex_2026-11-26/vecta_base_directory.json")).unwrap();
                        assert_eq!(proof["selector"],snapshot["selector"]);
                        assert_eq!(proof["init_hash"].as_str(),Some(text.as_str()));
                    } else if k=="dex.pool_code_hashes" && p.id=="communityv3" {
                        let deployment:Value=serde_json::from_str(include_str!("../../tests/data/first_run_dex_2026-11-26/community_v3_ethereum_router.json")).unwrap();
                        assert_eq!(deployment["selector"],snapshot["selector"]);assert_eq!(deployment["init"].as_str(),Some(text.as_str()));
                    } else if k=="dex.pool_code_hashes" && p.id=="synthswap-v3" {
                        let raw=include_bytes!("../../tests/data/first_run_dex_2026-11-26/synth_oracle_base.json.gz");
                        let proof:Value=serde_json::from_reader(flate2::read::GzDecoder::new(raw.as_slice())).unwrap();
                        assert_eq!(proof["selector"],snapshot["selector"]);
                        assert_eq!(proof["init_code_hash"].as_str(),Some(text.as_str()));
                    } else if k == "dex.pool_code_hashes" && matches!(p.id.as_str(),"unicly"|"hopeswap"|"archly-ethereum") {
                        let raw:&[u8]=if p.id=="archly-ethereum"{include_bytes!("../../tests/data/first_run_dex_2026-11-26/archly_ethereum_wallets.json.gz")}else if p.id=="unicly"{include_bytes!("../../tests/data/first_run_dex_2026-11-26/unicly_ethereum_wallets.json.gz")}else{include_bytes!("../../tests/data/first_run_dex_2026-11-26/hopeswap_ethereum_wallets.json.gz")};
                        let proof:Value=serde_json::from_reader(flate2::read::GzDecoder::new(raw)).unwrap();
                        assert_eq!(proof["selector"],snapshot["selector"]);
                        assert_eq!(proof["pair_init"].as_str(),Some(text.as_str()));
                    } else if k == "dex.pool_code_hashes" && p.id == "toonswap" {
                        let raw=include_bytes!("../../tests/data/first_run_dex_2026-11-26/toonswap_ethereum_wallets.json.gz");
                        let proof:Value=serde_json::from_reader(flate2::read::GzDecoder::new(raw.as_slice())).unwrap();
                        assert_eq!(proof["selector"],snapshot["selector"]);
                        assert_eq!(proof["pair_init"].as_str(),Some(text.as_str()));
                    } else if k == "dex.pool_code_hashes" && p.id == "swapr_ethereum" {
                        let raw=include_bytes!("../../tests/data/first_run_dex_2026-11-26/swapr_ethereum.json.gz");
                        let proof:Value=serde_json::from_reader(flate2::read::GzDecoder::new(raw.as_slice())).unwrap();
                        assert_eq!(proof["selector"],snapshot["selector"]);
                        assert_eq!(proof["chain_id"],p.chain_id);
                        assert_eq!(proof["creation_code_hash"].as_str(),Some(text.as_str()));
                    } else if k == "dex.pool_code_hashes" && p.id == "memebox-base" {
                        let archive:Value=serde_json::from_str(include_str!("../../tests/data/first_run_dex_2026-11-26/memebox_base.json")).unwrap();
                        assert_eq!(archive["selector"],snapshot["selector"]);
                        assert_eq!(archive["creation_hash"].as_str(),Some(text.as_str()));
                    } else if k == "dex.pool_code_hashes" && p.id == "oasisswap-base" {
                        let archive:Value=serde_json::from_str(include_str!("../../tests/data/first_run_dex_2026-11-26/oasisswap_base.json")).unwrap();
                        assert_eq!(archive["selector"],snapshot["selector"]);
                        assert_eq!(archive["init"].as_str(),Some(text.as_str()));
                    } else if k == "dex.pool_code_hashes" && matches!(p.settings.get("dex.kind"),Some(Parameter::Text(kind)) if kind=="v2_deltaswap_router") {
                        let capture = match p.chain_id {
                            1 => include_str!("../../tests/data/first_run_dex_2026-11-26/deltaswap_ethereum.json"),
                            8453 => include_str!("../../tests/data/first_run_dex_2026-11-26/deltaswap_base.json"),
                            id => panic!("нет записи DeltaSwap для chainId {id}"),
                        };
                        let archive:Value=serde_json::from_str(capture).unwrap();
                        assert_eq!(archive["current"]["selector"], snapshot["selector"]);
                        assert_eq!(
                            archive["current"]["proof"]["pair_init"].as_str(),
                            Some(text.as_str())
                        );
                    } else if k=="dex.pool_code_hashes" && matches!(p.id.as_str(),"empire-module-ethereum"|"empire-nominal-ethereum"){
                        let records:Value=serde_json::from_reader(flate2::read::GzDecoder::new(&include_bytes!("../../tests/data/first_run_dex_2026-11-26/llama_eth_v2_wallets.json.gz")[..])).unwrap();
                        let basis=&records["empire"];
                        assert_eq!(basis["selector"],snapshot["selector"]);
                        assert_eq!(p.settings.get("dex.factory"),Some(&Parameter::Text(basis["factory"].as_str().unwrap().to_owned())));
                        assert_eq!(basis["origin"]["creation_code_hash"].as_str(),Some(text.as_str()));
                    } else if k == "dex.pool_code_hashes" && p.id == "ring-exchange-ethereum" {
                        let directory: serde_json::Value = serde_json::from_str(include_str!(
                            "../../tests/data/first_run_dex_2026-11-26/ring_directory.json"
                        ))
                        .unwrap();
                        assert_eq!(directory["pair_init"].as_str(), Some(text.as_str()));
                    } else if k == "dex.pool_code_hashes" {
                        assert!(
                            pairs.iter().any(|row| row["network"] == p.network
                                && row["dex"] == p.id
                                && row["creation_code_hash"].as_str() == Some(text.as_str())),
                            "{} {k}: нет хеша создания пары {text} в записи",
                            p.id
                        );
                    } else if k == "dex.sub_dex" {
                        assert!(
                            all.iter()
                                .any(|child| child.network == p.network && &child.id == text)
                        );
                    }
                }
            }
        }
    }
}
