//! Связь исходной строки индексатора с областью настроек DEX.
//! Сопоставление происходит только при чтении внешнего справочника. Собственные записи фабрик уже имеют ключ DEX.
use super::{scope::Scope, snapshot::Values, store::Store};
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub(crate) struct Bindings {
    /// Ключи, для которых исходный ID отличается от имени области.
    renamed: HashMap<String, String>,
    /// Один внешний ID может принадлежать только одной области своей сети.
    by_source: HashMap<String, Option<String>>,
}

impl Bindings {
    pub(crate) fn read(settings: &Store) -> Self {
        Self::from_values(&settings.cache.load())
    }

    fn from_values(values: &Values) -> Self {
        let mut bindings = Self::default();
        for (scope, value) in values.named("dex.indexer_id") {
            let (Scope::Dex(key), Some(raw)) = (scope, value.as_str()) else {
                continue;
            };
            let external = match key.split_once('@') {
                Some((_, network)) => format!("{raw}@{network}"),
                None => raw.to_string(),
            };
            if key != &external {
                bindings.renamed.insert(key.clone(), external.clone());
            }
            bindings
                .by_source
                .entry(external)
                .and_modify(|claim| *claim = None)
                .or_insert_with(|| Some(key.clone()));
        }
        for (external, claim) in &mut bindings.by_source {
            if claim.as_ref().is_some_and(|s| s != external)
                && !bindings.renamed.contains_key(external)
                && values.has_scope(&Scope::Dex(external.clone()))
            {
                *claim = None;
            }
        }
        bindings
    }

    pub(crate) fn scope<'a>(&'a self, external: &'a str) -> Result<&'a str, String> {
        match self.by_source.get(external) {
            Some(Some(key)) => Ok(key),
            Some(None) => Err(format!(
                "Внешний ID {external} указан у нескольких областей DEX этой сети"
            )),
            None if self.renamed.contains_key(external) => Err(format!(
                "Внешний ID {external} не назначен: одноимённая область принимает другой ID"
            )),
            None => Ok(external),
        }
    }

    pub(crate) fn source<'a>(&'a self, key: &'a str) -> Option<&'a str> {
        let raw = self.renamed.get(key).map_or(key, String::as_str);
        (self.scope(raw).ok() == Some(key)).then_some(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn exact_ids_are_unique_per_network_and_rebind_without_rewriting_directory_rows() {
        let mut values = Values::default();
        let key = |s: &str| Scope::Dex(s.to_string());
        values.put(
            "dex.indexer_id",
            &key("foo-bar@base"),
            Some(json!("foo.bar")),
        );
        values.put(
            "dex.indexer_id",
            &key("third@ethereum"),
            Some(json!("foo.bar")),
        );
        let first = Bindings::from_values(&values);
        assert_eq!(first.scope("foo.bar@base").unwrap(), "foo-bar@base");
        assert!(first.scope("foo-bar@base").is_err());
        assert_eq!(first.scope("foo.bar@ethereum").unwrap(), "third@ethereum");
        assert_eq!(first.scope("legacy").unwrap(), "legacy");
        values.put(
            "dex.indexer_id",
            &key("independent@base"),
            Some(json!("foo-bar")),
        );
        let distinct = Bindings::from_values(&values);
        assert_eq!(distinct.scope("foo-bar@base").unwrap(), "independent@base");
        assert_eq!(distinct.source("foo-bar@base"), Some("foo.bar@base"));
        values.put(
            "dex.indexer_id",
            &key("fourth@base"),
            Some(json!("foo.bar")),
        );
        assert!(
            Bindings::from_values(&values)
                .scope("foo.bar@base")
                .is_err()
        );
        values.put("dex.indexer_id", &key("fourth@base"), Some(json!("legacy")));
        values.put("dex.kind", &key("legacy@base"), Some(json!("v2")));
        assert!(Bindings::from_values(&values).scope("legacy@base").is_err());
        values.put("dex.indexer_id", &key("foo-bar@base"), None);
        assert_eq!(
            Bindings::from_values(&values)
                .scope("foo.bar@base")
                .unwrap(),
            "foo.bar@base"
        );
    }
}
