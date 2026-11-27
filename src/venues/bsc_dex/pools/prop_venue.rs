//! Канонический реестр LambdaClass V1. Конкретный рынок и касса подписаны в профиле DEX.
//! https://github.com/lambdaclass/propamm-router-contracts
mod kipseli;
use super::{
    families::{self, Family},
    family_variant::Variant,
    nami_batch as batch, solidly_classic_factory as code,
    stable_list::Extra,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        exec::{
            carbon_binding::Code,
            prop_abi::{IProp, IStrict},
            prop_wallet::Terms,
        },
    },
};
use alloy_primitives::{Address, B256};
use serde_json::Value;
use std::collections::HashMap;
pub(crate) fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "LambdaClass: не подтверждены сеть, рынок, касса или условия выплаты",
    )
}
#[derive(Clone, Copy)]
struct Role {
    address: Address,
    hash: B256,
    implementation: Option<(Address, B256)>,
}
impl Role {
    async fn check(self, ctx: &Ctx, at: &Value) -> GatewayResult<()> {
        code::bind(ctx, self.address, self.hash, self.implementation, at).await
    }
    fn proof(self) -> Option<Code> {
        let (implementation, runtime) = self.implementation?;
        Some(Code {
            shell: self.hash,
            implementation,
            runtime,
        })
    }
}
pub(crate) struct Roles {
    router: Role,
    venue: Role,
    vault: Role,
    fallback: Role,
    intermediaries: Option<[Role; 3]>,
}
impl Roles {
    pub(crate) fn read(ctx: &Ctx, id: &str) -> GatewayResult<Self> {
        let rows = ctx.dex_list("dex.propamm_roles", id)?;
        let read = |key| -> GatewayResult<Role> {
            let mut found = rows
                .iter()
                .map(|row| row.split_whitespace().collect::<Vec<_>>())
                .filter(|f| f.first() == Some(&key));
            let f = found.next().ok_or_else(|| bad(ctx))?;
            if found.next().is_some() || !matches!(f.len(), 3 | 5) {
                return Err(bad(ctx));
            }
            let addr = |s: &str| {
                s.parse::<Address>()
                    .ok()
                    .filter(|a| !a.is_zero())
                    .ok_or_else(|| bad(ctx))
            };
            let hash = |s: &str| {
                s.parse::<B256>()
                    .ok()
                    .filter(|a| !a.is_zero() && *a != alloy_primitives::keccak256([]))
                    .ok_or_else(|| bad(ctx))
            };
            Ok(Role {
                address: addr(f[1])?,
                hash: hash(f[2])?,
                implementation: if f.len() == 5 {
                    Some((addr(f[3])?, hash(f[4])?))
                } else {
                    None
                },
            })
        };
        let s = Self {
            router: read("router")?,
            venue: read("venue")?,
            vault: read("vault")?,
            fallback: read("fallback")?,
            intermediaries: match rows.len() {
                4 => None,
                7 => Some([read("pool")?, read("logic")?, read("singleton")?]),
                _ => return Err(bad(ctx)),
            },
        };
        if s.router.proof().is_none()
            || s.router.address != ctx.dex_addr("dex.quoter", id)?
            || s.venue.address != ctx.dex_addr("dex.factory", id)?
            || s.venue.address == s.fallback.address
        {
            return Err(bad(ctx));
        }
        Ok(s)
    }
    pub(crate) fn quoted_venue(&self) -> Address {
        self.venue.address
    }
    pub(crate) fn fallback(&self) -> Address {
        self.fallback.address
    }
    pub(crate) fn router(&self) -> Address {
        self.router.address
    }
    pub(crate) fn terms(
        &self,
        ctx: &Ctx,
        target: Address,
        fallback_allowed: bool,
    ) -> GatewayResult<Terms> {
        Ok(Terms {
            target,
            fallback_allowed,
            router: self.router.address,
            venue: self.venue.address,
            vault: self.vault.address,
            fallback: self.fallback.address,
            code: self.router.proof().ok_or_else(|| bad(ctx))?,
            venue_code: self.venue.proof(),
            vault_code: self.vault.proof(),
            via: self
                .intermediaries
                .map(|[pool, logic, _]| [pool.address, logic.address]),
        })
    }
}
fn pairs(ctx: &Ctx, id: &str) -> GatewayResult<Vec<(Address, Address)>> {
    ctx.dex_list("dex.propamm_pairs", id)?
        .iter()
        .map(|row| {
            let mut f = row.split_whitespace();
            let (Some(a), Some(b), None) = (f.next(), f.next(), f.next()) else {
                return Err(bad(ctx));
            };
            let a = a.parse::<Address>().map_err(|_| bad(ctx))?;
            let b = b.parse::<Address>().map_err(|_| bad(ctx))?;
            if a == b || a.is_zero() || b.is_zero() {
                return Err(bad(ctx));
            }
            Ok((a.min(b), a.max(b)))
        })
        .collect()
}
pub(crate) async fn state(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    (a, b): (Address, Address),
    at: &Value,
) -> GatewayResult<(Roles, bool)> {
    let generation = ctx.settings.generation();
    let r = Roles::read(ctx, f.id)?;
    if r.venue.address != pool || !pairs(ctx, f.id)?.contains(&(a.min(b), a.max(b))) {
        return Err(bad(ctx));
    }
    let calls = [
        batch::view(at, r.router.address, IProp::pausedCall {}),
        batch::view(
            at,
            r.router.address,
            IProp::isWhitelistedVenueCall { venue: pool },
        ),
        batch::view(at, r.router.address, IProp::fallbackSwapRouterCall {}),
    ];
    let (v, (), (), (), (), ()) = tokio::try_join!(
        batch::read(ctx, calls),
        r.router.check(ctx, at),
        r.venue.check(ctx, at),
        r.vault.check(ctx, at),
        r.fallback.check(ctx, at),
        kipseli::state(ctx, &r, at)
    )?;
    if batch::decode(&v[2], IProp::fallbackSwapRouterCall {})? != r.fallback.address
        || generation != ctx.settings.generation()
    {
        return Err(bad(ctx));
    }
    let enabled = !batch::decode(&v[0], IProp::pausedCall {})?
        && batch::decode(&v[1], IProp::isWhitelistedVenueCall { venue: pool })?;
    Ok((r, enabled))
}
pub(crate) async fn module(
    ctx: &Ctx,
    f: &Family,
    r: &Roles,
    m: Address,
    at: &Value,
) -> GatewayResult<()> {
    if m != ctx.dex_addr("dex.router", f.id)? {
        return Err(bad(ctx));
    }
    let (row, ()) = tokio::try_join!(
        batch::read(
            ctx,
            [
                batch::view(at, m, IStrict::routerCall {}),
                batch::view(at, m, IStrict::marketCall {}),
                batch::view(at, m, IStrict::vaultCall {})
            ]
        ),
        super::direct_module::verify_code(ctx, f.id, m, at)
    )?;
    if [
        batch::decode(&row[0], IStrict::routerCall {})?,
        batch::decode(&row[1], IStrict::marketCall {})?,
        batch::decode(&row[2], IStrict::vaultCall {})?,
    ] != [r.router.address, r.venue.address, r.vault.address]
    {
        return Err(bad(ctx));
    }
    Ok(())
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let check = async {
        let p = c.pool.parse::<Address>().map_err(|_| bad(ctx))?;
        let (token0, token1) = super::discover::sorted(c.coin, c.quote);
        let at = super::nami::block(ctx, "latest").await?;
        state(ctx, c.family, p, (token0, token1), &at).await?;
        Ok::<_, crate::gateway::errors::GatewayError>(PoolRef::Multi {
            contract: p,
            token0,
            token1,
        })
    }
    .await;
    match check {
        Ok(p) => Verdict::Taken(Box::new(super::verify_woo::found(c, p))),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
pub(super) fn directory(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut out = Extra::default();
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| f.variant == Variant::PropVenue)
    {
        if let (Ok(r), Ok(accepted)) = (Roles::read(ctx, f.id), pairs(ctx, f.id)) {
            for (a, b) in accepted {
                for row in
                    super::stable_list::candidates(f.id, r.venue.address, &[a, b], coins, quotes)
                {
                    out.found.entry(row.base).or_default().push(row)
                }
            }
        }
    }
    out
}
