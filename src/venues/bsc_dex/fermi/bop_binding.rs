//! Исходный профиль Ethereum, который подписчик проверяет на родителе, а восстановление — на блоке выплаты.
//! Адреса и хеши взяты из свойств конкретной площадки, проверенные рекомендации происходят из двух канонических снимков.
use super::{
    Ctx,
    bopamm_profile::{Profile, Role},
    canonical,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::settings::{recommendations, scope::Scope};
use alloy::primitives::{Address, B256, aliases::U24};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Cash {
    pub weth: Role,
    pub usdc: Role,
    pub usdt: Role,
    pub pool: Role,
    pub router: Role,
    pub quoter: Role,
    pub factory: Role,
    pub fee: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Identity {
    pub market: Role,
    pub pricing: Role,
    pub registry: Role,
    pub cash: Cash,
}
impl Cash {
    fn parse(rows: &Value) -> Option<Self> {
        let mut bindings = BTreeMap::new();
        let mut unique = BTreeSet::new();
        let mut fee = None;
        for row in rows.as_array()? {
            let words = row.as_str()?.split_whitespace().collect::<Vec<_>>();
            match words.as_slice() {
                ["fee", raw] if fee.is_none() => {
                    fee = Some(
                        raw.parse::<u32>()
                            .ok()
                            .filter(|n| *n > 0 && *n < 1_000_000)?,
                    )
                }
                [name, address, hash]
                    if matches!(
                        *name,
                        "weth" | "usdc" | "usdt" | "pool" | "router" | "quoter" | "factory"
                    ) =>
                {
                    let role = Role {
                        address: address.parse().ok()?,
                        hash: hash.parse().ok()?,
                    };
                    if role.address.is_zero()
                        || role.hash.is_zero()
                        || !unique.insert(role.address)
                        || bindings.insert(*name, role).is_some()
                    {
                        return None;
                    }
                }
                _ => return None,
            }
        }
        Some(Self {
            weth: bindings.remove("weth")?,
            usdc: bindings.remove("usdc")?,
            usdt: bindings.remove("usdt")?,
            pool: bindings.remove("pool")?,
            router: bindings.remove("router")?,
            quoter: bindings.remove("quoter")?,
            factory: bindings.remove("factory")?,
            fee: fee?,
        })
    }
}
impl Identity {
    pub(super) fn from(profile: &Profile, cash: Cash) -> Self {
        Self {
            market: profile.market,
            pricing: profile.pricing,
            registry: profile.registry,
            cash,
        }
    }
    pub(super) fn issued() -> Option<&'static Self> {
        static VERIFIED: LazyLock<Option<Identity>> = LazyLock::new(|| {
            let venue = crate::settings::evm_profiles::PROFILES
                .iter()
                .find(|p| p.evm_chain_id == crate::settings::evm_profiles::ETHEREUM_MAINNET)?
                .venue;
            let rows = |key| {
                recommendations::venue_value(key, venue)
                    .map(|s| json!(s.split(',').collect::<Vec<_>>()))
            };
            Some(Identity::from(
                &Profile::parse(&rows("bopamm.roles")?)?,
                Cash::parse(&rows("bopamm.cash_path")?)?,
            ))
        });
        VERIFIED.as_ref()
    }
    pub(super) fn configured(ctx: &Ctx, p: &Profile) -> Option<&'static Self> {
        let rows = ctx
            .settings
            .get_exact("bopamm.cash_path", &Scope::Venue(ctx.place.id.into()))?;
        let local = Self::from(p, Cash::parse(&rows)?);
        Self::issued().filter(|i| **i == local)
    }
    pub(super) fn profile(self, module: Role) -> Profile {
        Profile {
            market: self.market,
            pricing: self.pricing,
            registry: self.registry,
            module: Some(module),
            cash_module: None,
        }
    }
    pub(super) async fn verify(
        &self,
        ctx: &Ctx,
        h: B256,
        module: Role,
        has_bridge: bool,
    ) -> GatewayResult<()> {
        let c = &self.cash;
        let fail = || {
            ctx.err(
                ErrorCode::NotApplicable,
                "bopAMM: изменились роли рынка, кассы или контракта пула",
            )
        };
        let codes = async {
            futures_util::future::try_join_all(
                [
                    c.weth, c.usdc, c.usdt, c.pool, c.router, c.quoter, c.factory,
                ]
                .into_iter()
                .map(|r| super::bopamm::verify_role(ctx, h, r)),
            )
            .await?;
            Ok(())
        };
        let profile = self.profile(module);
        let (_, _, pair, fees, left, right, rf, qf, pf, settlement) = tokio::try_join!(
            profile.verify(ctx, h, Some(module)),
            codes,
            canonical(
                ctx,
                h,
                c.factory.address,
                getPoolCall {
                    a: c.usdc.address,
                    b: c.usdt.address,
                    tier: U24::from(c.fee)
                }
            ),
            canonical(ctx, h, c.pool.address, feeCall {}),
            canonical(ctx, h, c.pool.address, token0Call {}),
            canonical(ctx, h, c.pool.address, token1Call {}),
            canonical(ctx, h, c.router.address, factoryCall {}),
            canonical(ctx, h, c.quoter.address, factoryCall {}),
            canonical(ctx, h, c.pool.address, factoryCall {}),
            canonical(ctx, h, module.address, super::bopamm::usdcCall {})
        )?;
        if (pair, fees, left, right, rf, qf, pf, settlement)
            != (
                c.pool.address,
                U24::from(c.fee),
                c.usdc.address,
                c.usdt.address,
                c.factory.address,
                c.factory.address,
                c.factory.address,
                c.usdc.address,
            )
        {
            return Err(fail());
        }
        if has_bridge {
            let a = module.address;
            let (weth, usdc, usdt, router, quoter, tier) = tokio::try_join!(
                canonical(ctx, h, a, WETHCall {}),
                canonical(ctx, h, a, USDCCall {}),
                canonical(ctx, h, a, USDTCall {}),
                canonical(ctx, h, a, V3_ROUTERCall {}),
                canonical(ctx, h, a, V3_QUOTERCall {}),
                canonical(ctx, h, a, POOL_FEECall {})
            )?;
            if (weth, usdc, usdt, router, quoter, tier)
                != (
                    c.weth.address,
                    c.usdc.address,
                    c.usdt.address,
                    c.router.address,
                    c.quoter.address,
                    U24::from(c.fee),
                )
            {
                return Err(fail());
            }
        }
        Ok(())
    }
}
alloy::sol! {
    function WETH() external view returns(address);
    function USDC() external view returns(address);
    function USDT() external view returns(address);
    function V3_ROUTER() external view returns(address);
    function V3_QUOTER() external view returns(address);
    function POOL_FEE() external view returns(uint24);
    function factory() external view returns(address);
    function token0() external view returns(address);
    function token1() external view returns(address);
    function fee() external view returns(uint24);
    function getPool(address a,address b,uint24 tier) external view returns(address);
}
