//! Действующие роли PCSX из настроек площадки и каноническая привязка подписываемого входа.
use super::Roles;
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{abi::IERC20, call::canonical},
        pools::solidly_classic_factory::{bind, plain},
        quote::white_state::Head,
    },
};
use alloy_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};

alloy::sol! {
    function permit2() view returns(address);
    function exclusiveDutchReactor() view returns(address);
    function v3DutchReactor() view returns(address);
    function nonceBitmap(address owner,uint256 wordPos) view returns(uint256);
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Role {
    pub address: Address,
    pub runtime: B256,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub chain: u64,
    pub router: Role,
    pub implementation: Role,
    pub permit: Role,
    pub v1: Role,
    pub v3: Role,
}
impl Profile {
    pub fn from_rows(chain: u64, rows: &[String]) -> Option<Self> {
        let mut roles = [None; 5];
        for row in rows {
            let f: Vec<_> = row.split_whitespace().collect();
            let [network, name, addr, hash] = f.as_slice() else {
                return None;
            };
            if network.parse::<u64>().ok()? != chain {
                continue;
            }
            let i = ["router", "implementation", "permit", "v1", "v3"]
                .iter()
                .position(|s| s == name)?;
            let r = Role {
                address: addr.parse().ok()?,
                runtime: hash.parse().ok()?,
            };
            if r.address.is_zero()
                || r.runtime.is_zero()
                || roles
                    .iter()
                    .flatten()
                    .any(|v: &Role| v.address == r.address)
                || roles[i].replace(r).is_some()
            {
                return None;
            }
        }
        let [
            Some(router),
            Some(implementation),
            Some(permit),
            Some(v1),
            Some(v3),
        ] = roles
        else {
            return None;
        };
        Some(Self {
            chain,
            router,
            implementation,
            permit,
            v1,
            v3,
        })
    }
    pub fn roles(&self) -> Roles {
        Roles {
            chain: self.chain,
            reactor: self.router.address,
            permit: self.permit.address,
        }
    }
    pub fn read(ctx: &Ctx) -> Option<Self> {
        let rows = ctx.settings.list("pancakex.contracts", ctx.at()).ok()?;
        Self::from_rows(ctx.chain_id()?, &rows)
    }
    pub(crate) async fn check(&self, ctx: &Ctx, h: Head, version: &str) -> GatewayResult<()> {
        let at = h.selector();
        let bad = || {
            ctx.err(
                ErrorCode::QuoteMismatch,
                "PancakeSwap X: роли подписи и исполняемого реактора изменились",
            )
        };
        if ctx.chain_id() != Some(self.chain) {
            return Err(bad());
        }
        let (facet, advertised) = match version {
            "v1" => (
                self.v1,
                canonical(ctx, &at, self.router.address, exclusiveDutchReactorCall {}).await?,
            ),
            "v3" => (
                self.v3,
                canonical(ctx, &at, self.router.address, v3DutchReactorCall {}).await?,
            ),
            _ => return Err(bad()),
        };
        let (_, _, _, related) = tokio::try_join!(
            bind(
                ctx,
                self.router.address,
                self.router.runtime,
                Some((self.implementation.address, self.implementation.runtime)),
                &at
            ),
            plain(ctx, self.permit.address, self.permit.runtime, &at),
            plain(ctx, facet.address, facet.runtime, &at),
            canonical(ctx, &at, self.router.address, permit2Call {})
        )?;
        if advertised != facet.address || related != self.permit.address {
            return Err(bad());
        }
        Ok(())
    }
    pub(crate) async fn unused(
        &self,
        ctx: &Ctx,
        h: Head,
        owner: Address,
        nonce: U256,
    ) -> GatewayResult<bool> {
        let n = canonical(
            ctx,
            &h.selector(),
            self.permit.address,
            nonceBitmapCall {
                owner,
                wordPos: nonce >> 8,
            },
        )
        .await?;
        Ok(!n.bit((nonce & U256::from(255)).to::<usize>()))
    }
    pub(crate) async fn funds(
        &self,
        ctx: &Ctx,
        h: Head,
        token: Address,
        who: Address,
    ) -> GatewayResult<U256> {
        let at = h.selector();
        let (b, a) = tokio::try_join!(
            canonical(ctx, &at, token, IERC20::balanceOfCall { owner: who }),
            canonical(
                ctx,
                &at,
                token,
                IERC20::allowanceCall {
                    owner: who,
                    spender: self.permit.address
                }
            )
        )?;
        Ok(b.min(a))
    }
}
