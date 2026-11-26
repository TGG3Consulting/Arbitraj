//! Файл: pools/virtuals_roles.rs. Проверяет фактические роли Bonding в настройках данного DEX на каноническом блоке.
//! Вызывается virtuals::market. Настройки: dex.bonding_roles, dex.quoter, dex.factory и хеши их реализаций.
use super::{families::Family, solidly_classic_factory};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, B256, keccak256};
use serde_json::Value;

#[derive(Clone, Copy)]
struct Role {
    address: Address,
    runtime: B256,
    implementation: Address,
    implementation_hash: B256,
}

impl Role {
    fn parse(row: &str) -> Option<Self> {
        let mut words = row.split_whitespace();
        let role = Self {
            address: words.next()?.parse().ok()?,
            runtime: words.next()?.parse().ok()?,
            implementation: words.next()?.parse().ok()?,
            implementation_hash: words.next()?.parse().ok()?,
        };
        (words.next().is_none()
            && !role.address.is_zero()
            && !role.implementation.is_zero()
            && role.address != role.implementation
            && !role.implementation_hash.is_zero()
            && role.implementation_hash != keccak256([]))
        .then_some(role)
    }
    async fn check(self, ctx: &Ctx, at: &Value) -> GatewayResult<()> {
        solidly_classic_factory::bind(
            ctx,
            self.address,
            self.runtime,
            Some((self.implementation, self.implementation_hash)),
            at,
        )
        .await
    }
}

pub(super) struct Roles {
    pub factory: Address,
    pub router: Address,
    pub front: Address,
    pub unicorn: bool,
}
pub(super) async fn at(ctx: &Ctx, f: &Family, at: &Value) -> GatewayResult<Roles> {
    let bad = || {
        ctx.err(ErrorCode::NotConfigured,"Virtuals: требуются отдельные реализации FRouter и публичного Bonding с хешами runtime")
    };
    let rows = ctx.dex_list("dex.bonding_roles", f.id)?;
    let [router, front] = rows.as_slice() else {
        return Err(bad());
    };
    let router = Role::parse(router).ok_or_else(bad)?;
    let front = Role::parse(front).ok_or_else(bad)?;
    let factory = solidly_classic_factory::at(ctx, f, at).await?;
    if ctx.dex_addr("dex.quoter", f.id)? != router.address
        || factory == router.address
        || factory == front.address
        || router.address == front.address
    {
        return Err(bad());
    }
    tokio::try_join!(router.check(ctx, at), front.check(ctx, at))?;
    let kind = ctx.dex_text("dex.kind", f.id)?;
    let unicorn = match kind.as_str() {
        "virtuals_bonding" | "virtuals_bonding_module" => false,
        "virtuals_unicorn" | "virtuals_unicorn_module" => true,
        _ => return Err(bad()),
    };
    Ok(Roles {
        factory,
        router: router.address,
        front: front.address,
        unicorn,
    })
}
