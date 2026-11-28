//! Deployment roles bound to a venue and frozen into each signed order.
use super::super::ctx::Ctx;
use crate::settings::scope::Scope;
use alloy_primitives::{Address, B256};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(in crate::venues::bsc_dex) struct Role {
    pub address: Address,
    pub runtime: B256,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(in crate::venues::bsc_dex) struct FrozenRoles {
    pub chain: u64,
    pub user_proxy: Role,
    pub proxy_impl: Role,
    pub pmm: Role,
    pub zero_exchange: Role,
    pub spender: Role,
    pub allowance_target: Role,
    pub weth: Role,
}

#[derive(Clone, Debug)]
pub(in crate::venues::bsc_dex) struct Asset {
    pub id: String,
    pub token: Role,
    pub impl_role: Option<Role>,
}

fn read_rows(ctx: &Ctx, name: &str) -> Option<Vec<String>> {
    let setting = ctx
        .settings
        .get_exact(name, &Scope::Venue(ctx.place.id.into()))?;
    setting
        .as_array()?
        .iter()
        .map(|row| Some(row.as_str()?.to_owned()))
        .collect()
}

fn role(fields: &[&str]) -> Option<Role> {
    let [addr, code] = fields else {
        return None;
    };
    let address: Address = addr.parse().ok()?;
    let runtime: B256 = code.parse().ok()?;
    (!address.is_zero() && !runtime.is_zero()).then_some(Role { address, runtime })
}

impl FrozenRoles {
    pub(in crate::venues::bsc_dex) fn read(ctx: &Ctx) -> Option<Self> {
        Self::from_rows(ctx.chain_id()?, &read_rows(ctx, "tokenlon.contracts")?)
    }

    pub(in crate::venues::bsc_dex) fn from_rows(chain: u64, rows: &[String]) -> Option<Self> {
        let mut roles = HashMap::new();
        let mut addresses = HashSet::new();
        for row in rows {
            let fields: Vec<_> = row.split_whitespace().collect();
            let [net, name, addr, code] = fields[..] else {
                return None;
            };
            if net.parse::<u64>().ok()? != chain {
                continue;
            }
            let value = role(&[addr, code])?;
            if chain == 0 || !addresses.insert(value.address) || roles.insert(name, value).is_some()
            {
                return None;
            }
        }
        if roles.len() != 7 {
            return None;
        }
        let frozen = Self {
            chain,
            user_proxy: *roles.get("user_proxy")?,
            proxy_impl: *roles.get("proxy_impl")?,
            pmm: *roles.get("pmm")?,
            zero_exchange: *roles.get("zero_exchange")?,
            spender: *roles.get("spender")?,
            allowance_target: *roles.get("allowance_target")?,
            weth: *roles.get("weth")?,
        };
        frozen.verified().then_some(frozen)
    }
    pub(in crate::venues::bsc_dex) fn verified(&self) -> bool {
        use alloy_primitives::b256;
        self.chain != 0
            && [
                (
                    self.user_proxy,
                    b256!("abf73cab47631c9363fa11b421585b9edc25458176a4e00292846ed8a45b3c1a"),
                ),
                (
                    self.proxy_impl,
                    b256!("db3bf7e4d3bea9449bdaaf049140386c9c0d57c1d9fcb962a88310b002fc70a5"),
                ),
                (
                    self.pmm,
                    b256!("d93c60d37531f7091af0f56e1ead05e61b26fffe245618ea084d807b00a5ab90"),
                ),
                (
                    self.zero_exchange,
                    b256!("1eb08f4905a226eb84c664ee73b6d81cd1a40720e6b3dd946591500a959aeefa"),
                ),
                (
                    self.spender,
                    b256!("fe937cf0bad2d1933b69b508ad8e501ef2b1f6fcbb5be1782dfd76c38f27718e"),
                ),
                (
                    self.allowance_target,
                    b256!("30faf08d24ed7a3fb9467a35c72e410cac980e7a0477e241a9427928efcbfb64"),
                ),
                (
                    self.weth,
                    b256!("d0a06b12ac47863b5c7be4185c2deaad1c61557033f56c7d4ea74429cbb25e23"),
                ),
            ]
            .into_iter()
            .all(|(role, code)| !role.address.is_zero() && role.runtime == code)
    }
}

pub(super) fn asset_rows(chain: u64, rows: &[String]) -> Option<Vec<Asset>> {
    let mut assets = Vec::new();
    let mut seen = HashSet::new();
    for row in rows {
        let fields: Vec<_> = row.split_whitespace().collect();
        let [net, name, addr, hash, ref remaining @ ..] = fields[..] else {
            return None;
        };
        if net.parse::<u64>().ok()? != chain {
            continue;
        }
        let token = role(&[addr, hash])?;
        if !seen.insert(token.address) {
            return None;
        }
        let impl_role = match (name, remaining) {
            ("fiat-usdc", [addr, hash]) => Some(role(&[addr, hash])?),
            ("tether-net" | "exact-wbtc" | "exact-uni" | "wrapped-native", []) => None,
            _ => return None,
        };
        assets.push(Asset {
            id: name.to_owned(),
            token,
            impl_role,
        });
    }
    (!assets.is_empty()).then_some(assets)
}

pub(in crate::venues::bsc_dex) fn assets(ctx: &Ctx) -> Option<Vec<Asset>> {
    asset_rows(ctx.chain_id()?, &read_rows(ctx, "tokenlon.tokens")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Deserialize)]
    struct Record {
        chain: u64,
        roles: Vec<String>,
        assets: Vec<String>,
    }
    #[test]
    fn first_run_matches_all_recorded_contracts_and_assets() {
        let record: Record = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_roles.json"
        ))
        .unwrap();
        for (key, rows) in [
            ("tokenlon.contracts", &record.roles),
            ("tokenlon.tokens", &record.assets),
        ] {
            let setting = crate::settings::recommendations::venue_value(key, "eth_dex").unwrap();
            assert_eq!(setting.split(',').collect::<Vec<_>>(), *rows);
        }
        let frozen = FrozenRoles::from_rows(record.chain, &record.roles).unwrap();
        let tokens = asset_rows(record.chain, &record.assets).unwrap();
        assert_eq!(
            tokens
                .iter()
                .find(|a| a.id == "wrapped-native")
                .unwrap()
                .token,
            frozen.weth
        );
        assert!(
            tokens
                .iter()
                .find(|a| a.id == "fiat-usdc")
                .unwrap()
                .impl_role
                .is_some()
        );
        assert!(FrozenRoles::from_rows(record.chain + 1, &record.roles).is_none());
        let mut duplicate = record.roles.clone();
        duplicate.push(record.roles[0].clone());
        assert!(FrozenRoles::from_rows(record.chain, &duplicate).is_none());
        let mut altered = record.roles.clone();
        let h = altered[0].split_whitespace().last().unwrap().to_owned();
        altered[0] = altered[0].replace(&h, &frozen.pmm.runtime.to_string());
        assert!(FrozenRoles::from_rows(record.chain, &altered).is_none());
    }
}
