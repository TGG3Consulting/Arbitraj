//! Файл: venues/bsc_dex/pools/deployment_profile.rs.
//! Что делает: индекс записей приёмки DEX с адресами и сетью из справочника первого запуска.
//! Кто вызывает: прямые модули, валидаторы профиля и проверка изменяемой комиссии V2.
//! Настройки: рабочие значения читают вызывающие адаптеры. Индекс сверяет их с записью приёмки.
use alloy_primitives::Address;
use serde_json::Value;
use std::{collections::BTreeMap, sync::OnceLock};

struct Catalog {
    rows: Vec<Value>,
    names: BTreeMap<u64, BTreeMap<String, usize>>,
    factories: BTreeMap<(u64, Address), BTreeMap<String, Option<usize>>>,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let rows: Vec<Value> =
            serde_json::from_str(include_str!("../../../../config/first_run_dex.json"))
                .expect("справочник DEX");
        let mut names = BTreeMap::<u64, BTreeMap<String, usize>>::new();
        let mut factories = BTreeMap::<(u64, Address), BTreeMap<String, Option<usize>>>::new();
        for (index, row) in rows.iter().enumerate() {
            let Some((chain, id)) = row["chain_id"].as_u64().zip(row["id"].as_str()) else {
                continue;
            };
            names.entry(chain).or_default().insert(id.to_owned(), index);
            let settings = &row["settings"];
            if let (Some(a), Some(k)) = (
                settings["dex.factory"]
                    .as_str()
                    .and_then(|a| a.parse::<Address>().ok()),
                settings["dex.kind"].as_str(),
            ) {
                let slot = factories
                    .entry((chain, a))
                    .or_default()
                    .entry(k.to_owned())
                    .or_insert(Some(index));
                if slot.is_some_and(|prior| rows[prior]["settings"] != *settings) {
                    *slot = None;
                }
            }
        }
        Catalog {
            rows,
            names,
            factories,
        }
    })
}

pub(super) fn rows() -> &'static [Value] {
    &catalog().rows
}

pub fn named(chain: u64, id: &str) -> Option<&'static Value> {
    let c = catalog();
    let id = id.split_once('@').map_or(id, |(name, _)| name);
    c.rows.get(*c.names.get(&chain)?.get(id)?)
}

pub(super) fn for_factory(chain: u64, factory: Address, kind: &str) -> Option<&'static Value> {
    let c = catalog();
    let index = (*c.factories.get(&(chain, factory))?.get(kind)?)?;
    c.rows.get(index)
}
