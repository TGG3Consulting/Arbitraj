//! Допуск ручной котировки bopAMM по физической сети поставляемого размещения и ролям оператора.
//! Для платёжной симуляции дополнительно проверяется размещённый владельцем BopAmmWallet.
use crate::settings::{evm_profiles, recommendations, scope::Scope, store::Store};
use alloy::primitives::{Address, B256};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

#[derive(Clone, Copy)]
pub(super) struct Role {
    pub(super) address: Address,
    pub(super) hash: B256,
}
pub(super) struct Profile {
    pub(super) market: Role,
    pub(super) pricing: Role,
    pub(super) registry: Role,
    pub(super) module: Option<Role>,
}
impl Profile {
    pub(super) fn parse(rows: &Value) -> Option<Self> {
        let mut roles = BTreeMap::new();
        let mut accounts = BTreeSet::new();
        for row in rows.as_array()? {
            let mut fields = row.as_str()?.split_whitespace();
            let role = fields.next()?;
            let address: Address = fields.next()?.parse().ok()?;
            let hash: B256 = fields.next()?.parse().ok()?;
            if !matches!(role, "market" | "pricing" | "registry" | "module")
                || fields.next().is_some()
                || address.is_zero()
                || hash.is_zero()
                || !accounts.insert(address)
                || roles.insert(role, Role { address, hash }).is_some()
            {
                return None;
            }
        }
        Some(Self {
            market: roles.remove("market")?,
            pricing: roles.remove("pricing")?,
            registry: roles.remove("registry")?,
            module: roles.remove("module"),
        })
    }

    /// Профиль поставки определяет сеть. Роли и необязательный модуль берутся из выбранной площадки.
    pub(super) fn read(settings: &Store, venue: &str) -> Option<(u64, Self)> {
        static DEPLOYED: LazyLock<Vec<u64>> = LazyLock::new(|| {
            evm_profiles::PROFILES
                .iter()
                .filter_map(|p| {
                    let rows = recommendations::venue_value("bopamm.roles", p.venue)?;
                    let roles = serde_json::json!(rows.split(',').collect::<Vec<_>>());
                    Profile::parse(&roles).map(|_| p.evm_chain_id)
                })
                .collect()
        });
        let chain = super::prop_profile::network_at(settings, venue)?;
        if !DEPLOYED.contains(&chain) {
            return None;
        }
        let rows = settings.get_exact("bopamm.roles", &Scope::Venue(venue.into()))?;
        Some((chain, Self::parse(&rows)?))
    }
}

/// Одинаковая доступность предпросмотра для домашнего кошелька и его строки сети.
pub(crate) fn configured(settings: &Store, venue: &str) -> bool {
    Profile::read(settings, venue).is_some()
}

#[cfg(test)]
#[path = "bopamm_profile_tests.rs"]
mod tests;
