//! Привязка кассы Textile, Permit2 и текущего получателя комиссии к одному каноническому блоку.
use super::wire::{IFee, IReactor, IWallet};
use crate::venues::bsc_dex::{
    ctx::Ctx,
    node::{call, rpc},
    quote::white_state::Head,
};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::json;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
pub(super) struct Profile {
    pub chain: u64,
    pub reactor: Address,
    pub taker: Address,
    pub collector: Address,
    pub bps: U256,
    pub protected: bool,
}
/// Строка прежнего wallet действует как общий выбор владельца площадки.
pub(super) fn roles(
    rows: &[String],
    chain: u64,
    owner: Address,
) -> Option<HashMap<&str, (Address, B256)>> {
    let mut roles = HashMap::new();
    let mut wallets = HashMap::new();
    for row in rows {
        let mut x = row.split_whitespace();
        if x.next()?.parse::<u64>().ok()? != chain {
            return None;
        }
        let role = x.next()?;
        let address = x.next()?.parse::<Address>().ok()?;
        let hash = x.next()?.parse::<B256>().ok()?;
        if address.is_zero() || hash.is_zero() || hash == keccak256([]) {
            return None;
        }
        if role == "wallet" {
            let payer = match x.next() {
                Some(word) => Some(word.parse::<Address>().ok().filter(|a| !a.is_zero())?),
                None => None,
            };
            if wallets.insert(payer, (address, hash)).is_some() {
                return None;
            }
        } else if !matches!(role, "reactor" | "fee" | "permit2")
            || roles.insert(role, (address, hash)).is_some()
        {
            return None;
        }
        if x.next().is_some() {
            return None;
        }
    }
    if let Some(&selected) = wallets.get(&Some(owner)).or_else(|| wallets.get(&None)) {
        roles.insert("wallet", selected);
    }
    Some(roles)
}
pub(super) async fn at(ctx: &Ctx, h: Head, owner: Address) -> Option<Profile> {
    let chain = ctx.chain_id()?;
    let rows = ctx.settings.list("textile.contracts", ctx.at()).ok()?;
    let roles = roles(&rows, chain, owner)?;
    let (reactor, hash) = *roles.get("reactor")?;
    let fee = roles.get("fee")?.0;
    let permit = roles.get("permit2")?.0;
    let tag = h.selector();
    for (address, expected) in roles.values() {
        let code: Bytes = rpc::call(ctx, "eth_getCode", json!([address, tag]))
            .await
            .ok()?;
        if keccak256(code) != *expected {
            return None;
        }
    }
    let (actual_fee, actual_permit, bps, collector) = tokio::try_join!(
        call::canonical(ctx, &tag, reactor, IReactor::feeControllerCall {}),
        call::canonical(ctx, &tag, reactor, IReactor::permit2Call {}),
        call::canonical(ctx, &tag, fee, IFee::FEE_BPSCall {}),
        call::canonical(ctx, &tag, fee, IFee::feeRecipientCall {})
    )
    .ok()?;
    if (actual_fee, actual_permit) != (fee, permit)
        || collector.is_zero()
        || collector == owner
        || bps.is_zero()
        || bps > U256::from(5)
    {
        return None;
    }
    let taker = if let Some(&(w, _)) = roles.get("wallet") {
        let (payer, registered, expected) = tokio::try_join!(
            call::canonical(ctx, &tag, w, IWallet::ownerCall {}),
            call::canonical(ctx, &tag, w, IWallet::reactorCall {}),
            call::canonical(ctx, &tag, w, IWallet::reactorCodeHashCall {})
        )
        .ok()?;
        if payer != owner || registered != reactor || expected != hash || w == collector {
            return None;
        }
        w
    } else {
        owner
    };
    Some(Profile {
        chain,
        reactor,
        taker,
        collector,
        bps,
        protected: taker != owner,
    })
}
