//! Файл: tests/common/first_run_channel_presets.rs
//! Что делает: извлекает типизированные рекомендации нужных областей из двух поставляемых справочников.
//! Кто вызывает: tests/common/first_run_channels.rs. От чего зависит: штатная схема и валидатор настроек.
use super::sources;
use arb::settings::{catalog, schema::Kind, scope::Scope, value};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
type Entry = (String, String);
pub(super) type Presets = BTreeMap<Entry, Value>;

pub(super) fn input(kind: Kind, text: &str) -> Result<Value, String> {
    match kind {
        Kind::Bool => match text {
            "true" | "да" => Ok(json!(true)),
            "false" | "нет" => Ok(json!(false)),
            _ => Err(format!("недопустимое логическое значение {text}")),
        },
        Kind::Integer => text
            .parse::<u64>()
            .map(Value::from)
            .map_err(|e| e.to_string()),
        Kind::TextList | Kind::AddressList => Ok(json!(
            text.split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
        )),
        _ => Ok(json!(text)),
    }
}

pub(super) fn scope(label: &str) -> Option<Scope> {
    if label == "Global" {
        return Some(Scope::Global);
    }
    if let Some(id) = label.strip_prefix("Venue ")
        && matches!(id, "eth_dex" | "base_dex" | "kyberswap" | "bsc_dex")
    {
        return Some(Scope::Venue(id.into()));
    }
    let id = label.strip_prefix("DEX ")?;
    let (name, network) = id.rsplit_once('@')?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'-' | b'_'))
    {
        return None;
    }
    matches!(network, "base" | "ethereum").then(|| Scope::Dex(id.into()))
}

fn insert(values: &mut Presets, pair: Entry, value: Value) {
    if let Some(old) = values.insert(pair.clone(), value.clone()) {
        assert_eq!(
            old, value,
            "разные рекомендации первого запуска для {pair:?}"
        );
    }
}

/// Значения проверяются по той же (настройка, область), что получает UI, а не по одному имени.
pub(super) fn presets(required: &BTreeSet<String>, profiles: &[Value]) -> Presets {
    let mut values = Presets::new();
    let config: toml::Value = toml::from_str(include_str!("../../config/first_run.toml")).unwrap();
    for s in config["section"].as_array().unwrap() {
        for r in s
            .get("rec")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            let key = r["name"].as_str().unwrap();
            let (Some(def), Some(raw), Some(prefill)) = (
                catalog::find(key),
                r.get("value").and_then(toml::Value::as_str),
                r.get("prefill"),
            ) else {
                continue;
            };
            if !required.contains(key) {
                continue;
            }
            let label = match prefill["kind"].as_str() {
                Some("Global") => "Global".to_string(),
                Some("Venue") => format!("Venue {}", prefill["id"].as_str().unwrap()),
                _ => continue,
            };
            let Some(s) = scope(&label) else { continue };
            if label == "Venue bsc_dex"
                || (s == Scope::Global && arb::settings::evm_profiles::local_only(key))
            {
                continue;
            } // Только область старой площадки, самостоятельная настройка KyberSwap остаётся.
            let checked = value::validate(def, &s, &input(def.kind, raw).unwrap())
                .unwrap_or_else(|e| panic!("{key} {label} в TOML: {e}"));
            insert(&mut values, (key.to_owned(), label), checked);
        }
    }
    for p in profiles {
        let network = p["network"].as_str().unwrap();
        let venue = match network {
            "base" => "Venue base_dex",
            "ethereum" => "Venue eth_dex",
            _ => continue,
        };
        if let Some(common) = p["venue_settings"].as_object() {
            for (key, raw) in common.iter().filter(|(key, _)| required.contains(*key)) {
                insert_profile(&mut values, key, venue, raw);
            }
        }
        let area = format!("DEX {}@{network}", p["id"].as_str().unwrap());
        let full_profile = sources::selected_profile(p);
        let lambda = network == "ethereum" && p["settings"]["dex.kind"] == "lambda_propamm";
        for (key, raw) in p["settings"].as_object().unwrap() {
            let slots = lambda
                && matches!(
                    key.as_str(),
                    "dex.propamm_price_slots" | "dex.propamm_slot_builders"
                );
            if !(full_profile || slots) {
                continue;
            }
            // Parameter::display в src/api/first_run_dex.rs разрешает ID дочерних площадок в сети родителя.
            if key == "dex.sub_dex" {
                let qualified = json!(
                    raw.as_array()
                        .unwrap()
                        .iter()
                        .map(|v| format!("{}@{network}", v.as_str().unwrap()))
                        .collect::<Vec<_>>()
                );
                insert_profile(&mut values, key, &area, &qualified);
            } else {
                insert_profile(&mut values, key, &area, raw);
            }
        }
    }
    values
}

fn insert_profile(values: &mut Presets, key: &str, area: &str, raw: &Value) {
    let def = catalog::find(key).unwrap();
    let checked = value::validate(def, &scope(area).unwrap(), raw)
        .unwrap_or_else(|e| panic!("{key} {area} в профиле: {e}"));
    insert(values, (key.to_owned(), area.to_owned()), checked);
}
