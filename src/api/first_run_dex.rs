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
        let bitswap:Value=serde_json::from_str(include_str!("../../tests/data/first_run_dex_2026-11-26/bitswap_ethereum_registry.json")).unwrap();
        assert_eq!(bitswap["block_hash"],eth["selector"]["blockHash"]);
        assert_eq!(bitswap["chain"],eth["chain_id"]);
        pairs.push(serde_json::json!({"dex":"bitswap","network":"ethereum","creation_code_hash":bitswap["init"]}));
        for p in all {
            let snapshot = [&base, &eth]
                .into_iter()
                .find(|s| s["chain_id"] == p.chain_id)
                .unwrap();
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
                    if text.len() == 42 && text.starts_with("0x") {
                        assert!(
                            snapshot["contracts"][text.to_ascii_lowercase()]["bytes"]
                                .as_u64()
                                .is_some_and(|size| size > 0),
                            "{} / {} / {k}: нет записи о коде адреса {text}",
                            p.network,
                            p.id
                        );
                    } else if k == "dex.pool_code_hashes" && matches!(p.settings.get("dex.kind"),Some(Parameter::Text(kind)) if kind=="algebra_integral_supernova") {
                        let data:Value=serde_json::from_str(include_str!("../../tests/data/first_run_dex_2026-11-26/supernova_ethereum_directory.json")).unwrap();
                        assert_eq!(data["selector"],snapshot["selector"]);
                        assert_eq!(data["init"].as_str(),Some(text.as_str()));
                    } else if k == "dex.pool_code_hashes" && matches!(p.settings.get("dex.kind"),Some(Parameter::Text(kind)) if matches!(kind.as_str(),"algebra_integral_base_plugin_v1"|"algebra_integral_treble")) {
                        let path=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/data/first_run_dex_2026-11-26/base_algebra_{}.json.gz",p.id));
                        let file=std::fs::File::open(path).unwrap();
                        let data:Value=serde_json::from_reader(flate2::read::GzDecoder::new(file)).unwrap();
                        assert_eq!(data["selector"],snapshot["selector"]);
                        assert_eq!(data["reads"]["POOL_INIT_CODE_HASH()"].as_str(),Some(text.as_str()));
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
                    } else if k == "dex.pool_code_hashes" && p.id == "wagmi-ethereum" {
                        let evidence: Value=serde_json::from_str(include_str!("../../tests/data/first_run_dex_2026-11-26/wagmi_ethereum_registry.json")).unwrap();
                        assert_eq!(evidence["selector"],snapshot["selector"]);
                        assert_eq!(evidence["init"].as_str(),Some(text.as_str()));
                    } else if k=="dex.pool_code_hashes" && p.id=="synthswap-v3" {
                        let raw=include_bytes!("../../tests/data/first_run_dex_2026-11-26/synth_oracle_base.json.gz");
                        let proof:Value=serde_json::from_reader(flate2::read::GzDecoder::new(raw.as_slice())).unwrap();
                        assert_eq!(proof["selector"],snapshot["selector"]);
                        assert_eq!(proof["init_code_hash"].as_str(),Some(text.as_str()));
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
