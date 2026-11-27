//! Идентичность первичного рынка из настроек владельца. Строка с эмитентом разрешает конкретное внереестровое размещение.
use super::families::Family;
use crate::{
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, B256, keccak256};
use std::collections::BTreeMap;
#[derive(Clone, Debug)]
pub(super) struct Profile {
    pub cash: Address,
    pub share: Address,
    pub hub: Address,
    pub code: B256,
    pub hub_hash: B256,
    pub issuer: Option<Address>,
    pub average: bool,
    pub license_bps: u16,
}
pub(super) fn invalid(ctx: &Ctx) -> GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "Aktionariat: рынок, роли, код или формула не совпали с подтверждённым профилем",
    )
}
fn addr(v: &str) -> Option<Address> {
    v.parse::<Address>().ok().filter(|a| !a.is_zero())
}
fn hash(v: &str) -> Option<B256> {
    v.parse::<B256>()
        .ok()
        .filter(|h| !h.is_zero() && *h != keccak256([]))
}
fn parse(line: &str) -> Option<(Address, Profile)> {
    let fields: Vec<_> = line.split_whitespace().collect();
    let [market, cash, share, hub, code, hub_hash, origin, order, fee] = fields.as_slice() else {
        return None;
    };
    let (cash, share) = (addr(cash)?, addr(share)?);
    if cash == share {
        return None;
    }
    let issuer = if *origin == "registry" {
        None
    } else {
        Some(addr(origin)?)
    };
    let license_bps = fee.parse().ok().filter(|v: &u16| *v < 10000)?;
    Some((
        addr(market)?,
        Profile {
            cash,
            share,
            hub: addr(hub)?,
            code: hash(code)?,
            hub_hash: hash(hub_hash)?,
            issuer,
            average: match *order {
                "average" => true,
                "total" => false,
                _ => return None,
            },
            license_bps,
        },
    ))
}
pub(super) fn markets(ctx: &Ctx, f: &Family) -> GatewayResult<BTreeMap<Address, Profile>> {
    let mut out = BTreeMap::new();
    for item in ctx.dex_list("dex.broker_markets", f.id)? {
        let (address, row) = parse(&item).ok_or_else(|| invalid(ctx))?;
        if out.insert(address, row).is_some() {
            return Err(invalid(ctx));
        }
    }
    Ok(out)
}
