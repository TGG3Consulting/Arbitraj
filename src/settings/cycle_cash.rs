//! Закреплённая область расчёта замкнутого DEX-цикла. Отдельна от валюты котировок DEX и переводов CEX.
use super::{scope::Scope, store::Store};
use alloy_primitives::{Address, B256, keccak256};

pub const KEY: &str = "dex_dex.bridged_usdt";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Profile {
    pub chain: u64,
    pub token: Address,
    pub remote: Address,
    pub bridge: Address,
    pub runtime: B256,
}

pub fn read(s: &Store, venue: &str) -> Result<Option<Profile>, String> {
    let scope = Scope::Venue(venue.to_owned());
    let Some(value) = s.get_exact(KEY, &scope) else {
        return Ok(None);
    };
    let fail = || {
        format!(
            "{KEY} [{venue}]: нужны chainId, адрес USDT, исходный L1 USDT, мост и keccak256 runtime этой сети"
        )
    };
    let raw = value.as_str().ok_or_else(fail)?.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let mut words = raw.split_whitespace();
    let mut next = || words.next().ok_or_else(fail);
    let p = Profile {
        chain: next()?.parse().map_err(|_| fail())?,
        token: next()?.parse().map_err(|_| fail())?,
        remote: next()?.parse().map_err(|_| fail())?,
        bridge: next()?.parse().map_err(|_| fail())?,
        runtime: next()?.parse().map_err(|_| fail())?,
    };
    if words.next().is_some()
        || p.chain == 0
        || p.token.is_zero()
        || p.remote.is_zero()
        || p.bridge.is_zero()
        || p.bridge == p.token
        || p.remote == p.token
        || p.runtime.is_zero()
        || p.runtime == keccak256([])
        || s.get_exact("dex.evm_chain_id", &scope)
            .and_then(|v| v.as_u64())
            != Some(p.chain)
    {
        return Err(fail());
    }
    Ok(Some(p))
}
