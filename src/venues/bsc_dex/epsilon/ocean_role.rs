//! Адреса публичной заявки, её прокси и принятый пользователем модуль одной сети.
use super::{
    super::{
        ctx::Ctx,
        exec::carbon_binding::Code,
        node::rpc,
        pools::solidly_classic_factory::{bind, plain},
        quote::white_state::Head,
    },
    profile::Contract,
    wire::Plan,
};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::json;
use std::collections::BTreeMap;

pub(super) struct Roles {
    pub chain: u64,
    pub venue: Contract,
    pub ocean: Contract,
    pub implement: Contract,
    pub executor: Contract,
    pub wallet: Contract,
    pub funding: Option<super::ocean_funding::Funding>,
}
impl Roles {
    pub fn read(ctx: &Ctx) -> Option<Self> {
        let chain = ctx.chain_id()?;
        let mut rows = BTreeMap::new();
        for s in ctx
            .settings
            .list("epsilon.openocean_contracts", ctx.at())
            .ok()?
        {
            let mut it = s.split_whitespace();
            if it.next()?.parse::<u64>().ok()? != chain {
                return None;
            }
            let role = it.next()?.to_string();
            let address: Address = it.next()?.parse().ok()?;
            let code: B256 = it.next()?.parse().ok()?;
            if it.next().is_some()
                || address.is_zero()
                || code.is_zero()
                || code == keccak256([])
                || rows.insert(role, Contract { address, code }).is_some()
            {
                return None;
            }
        }
        if !matches!(rows.len(), 5 | 10)
            || rows
                .values()
                .map(|v| v.address)
                .collect::<std::collections::HashSet<_>>()
                .len()
                != rows.len()
        {
            return None;
        }
        let funding = if rows.len() == 10 {
            Some(super::ocean_funding::Funding {
                entry: rows.remove("funding")?,
                beacon: rows.remove("beacon")?,
                implementation: rows.remove("funding_implementation")?,
                vault: rows.remove("vault")?,
                vault_implementation: rows.remove("vault_implementation")?,
            })
        } else {
            None
        };
        Some(Self {
            funding,
            chain,
            venue: rows.remove("venue")?,
            ocean: rows.remove("provider")?,
            implement: rows.remove("implementation")?,
            executor: rows.remove("executor")?,
            wallet: rows.remove("wallet")?,
        })
    }
    pub fn code(&self) -> Code {
        Code {
            shell: self.ocean.code,
            implementation: self.implement.address,
            runtime: self.implement.code,
        }
    }
    pub async fn fee(&self, ctx: &Ctx, h: Head) -> Option<u16> {
        let r = self.venue.address;
        let provider = self.ocean.address.into_word();
        let (open, accept, fee) = tokio::join!(
            word(ctx, h, r, "isPaused()", &[]),
            word(ctx, h, r, "allowedTargets(address)", provider.as_slice()),
            word(ctx, h, r, "protocolFeeBps()", &[])
        );
        if open? != B256::ZERO || accept? != B256::from(U256::ONE) {
            return None;
        }
        u16::try_from(U256::from_be_slice(fee?.as_slice()))
            .ok()
            .filter(|f| *f < 10000)
    }
}
pub(super) async fn word(ctx: &Ctx, h: Head, to: Address, sig: &str, arg: &[u8]) -> Option<B256> {
    let mut data = keccak256(sig)[..4].to_vec();
    data.extend_from_slice(arg);
    rpc::call(
        ctx,
        "eth_call",
        json!([{"to":to,"data":Bytes::from(data)},h.selector()]),
    )
    .await
    .ok()
}
pub(super) async fn verified(ctx: &Ctx, h: Head, plan: &Plan) -> bool {
    let (Some(r), Some(api)) = (Roles::read(ctx), plan.ocean.as_ref()) else {
        return false;
    };
    if (plan.router, plan.wallet, api.proxy, api.executor)
        != (
            r.venue.address,
            Some(r.wallet.address),
            r.ocean.address,
            r.executor.address,
        )
        || (
            api.implementation.shell,
            api.implementation.implementation,
            api.implementation.runtime,
        ) != (r.ocean.code, r.implement.address, r.implement.code)
    {
        return false;
    }
    let at = h.selector();
    let w = r.wallet.address;
    let checks = tokio::try_join!(
        plain(ctx, r.venue.address, r.venue.code, &at),
        plain(ctx, r.executor.address, r.executor.code, &at),
        plain(ctx, w, r.wallet.code, &at),
        bind(
            ctx,
            r.ocean.address,
            r.ocean.code,
            Some((r.implement.address, r.implement.code)),
            &at
        )
    );
    if checks.is_err() {
        return false;
    }
    if api.prepaid(plan) != Some(false) {
        let Some(f) = api.funding.as_deref() else { return false };
        if Some(f) != r.funding.as_ref()
            || !f.at(ctx, h).await
            || word(ctx, h, w, "fundingReceiver()", &[]).await != Some(f.entry.address.into_word())
        {
            return false;
        }
    }
    let roles = tokio::join!(
        word(ctx, h, w, "router()", &[]),
        word(ctx, h, w, "ocean()", &[]),
        word(ctx, h, w, "executor()", &[])
    );
    roles
        == (
            Some(r.venue.address.into_word()),
            Some(r.ocean.address.into_word()),
            Some(r.executor.address.into_word()),
        )
        && (if api.nested {
            r.fee(ctx, h).await == Some(plan.fee_bps)
        } else {
            plan.fee_bps == 0
        })
}
