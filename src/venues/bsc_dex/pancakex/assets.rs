//! Точные физические единицы расчёта. B20 отдельно проверяет отправителя, получателя и контракт transferFrom.
use super::profile::Role;
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{call::canonical, rpc},
        pools::solidly_classic_factory::plain,
        ryze::tokens::Token,
    },
};
use alloy_primitives::{Address, Bytes, keccak256};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

alloy::sol! {
    function isB20(address token) view returns(bool);
    function isB20Initialized(address token) view returns(bool);
    function isPaused(uint8 feature) view returns(bool);
    function policyId(bytes32 scope) view returns(uint64);
    function policyExists(uint64 id) view returns(bool);
    function isAuthorized(uint64 id,address account) view returns(bool);
    function paused() view returns(bool);
    function deprecated() view returns(bool);
    function basisPointsRate() view returns(uint256);
    function isBlackListed(address account) view returns(bool);
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AssetCode {
    Exact(Role),
    Fiat(Token),
    B20 {
        token: Role,
        factory: Address,
        registry: Address,
    },
    TetherInput(Role),
}
impl AssetCode {
    pub fn parse(row: &str, chain: u64) -> Option<Self> {
        let f: Vec<_> = row.split_whitespace().collect();
        let [network, kind, address, hash, extra @ ..] = f.as_slice() else {
            return None;
        };
        if network.parse::<u64>().ok()? != chain {
            return None;
        }
        let r = Role {
            address: address.parse().ok()?,
            runtime: hash.parse().ok()?,
        };
        if r.address.is_zero() || r.runtime.is_zero() {
            return None;
        }
        match (*kind, extra) {
            ("exact", []) => Some(Self::Exact(r)),
            ("tether-input", []) => Some(Self::TetherInput(r)),
            ("fiat", [imp, ih, adm]) => Some(Self::Fiat(Token::parse(&format!(
                "{address}/{hash}/{imp}/{ih}/{adm}"
            ))?)),
            ("b20", [f, p]) => {
                let factory: Address = f.parse().ok()?;
                let registry: Address = p.parse().ok()?;
                (!factory.is_zero() && !registry.is_zero() && factory != registry).then_some(
                    Self::B20 {
                        token: r,
                        factory,
                        registry,
                    },
                )
            }
            _ => None,
        }
    }
    pub fn address(&self) -> Address {
        match self {
            Self::Exact(t) | Self::TetherInput(t) | Self::B20 { token: t, .. } => t.address,
            Self::Fiat(t) => t.address,
        }
    }
    pub fn can_receive(&self) -> bool {
        !matches!(self, Self::TetherInput(_))
    }
    pub async fn verify(
        &self,
        ctx: &Ctx,
        at: &Value,
        from: Address,
        to: Address,
        executor: Address,
    ) -> GatewayResult<()> {
        let token = self.address();
        let bad = || {
            ctx.err(
                ErrorCode::NotApplicable,
                "PancakeSwap X: изменился токен или условия перевода выбранного кошелька",
            )
        };
        match self {
            Self::Exact(r) => plain(ctx, token, r.runtime, at).await,
            Self::Fiat(t) => t.verify(ctx, at, &[from, to, executor]).await,
            Self::TetherInput(r) => {
                let (_, stopped, changed, rate, a, b) = tokio::try_join!(
                    plain(ctx, token, r.runtime, at),
                    canonical(ctx, at, token, pausedCall {}),
                    canonical(ctx, at, token, deprecatedCall {}),
                    canonical(ctx, at, token, basisPointsRateCall {}),
                    canonical(ctx, at, token, isBlackListedCall { account: from }),
                    canonical(ctx, at, token, isBlackListedCall { account: to })
                )?;
                if stopped || changed || !rate.is_zero() || a || b {
                    return Err(bad());
                }
                Ok(())
            }
            Self::B20 {
                token: t,
                factory,
                registry,
            } => {
                let (code, exists, ready, paused) = tokio::try_join!(
                    rpc::call::<Bytes>(ctx, "eth_getCode", json!([token, at])),
                    canonical(ctx, at, *factory, isB20Call { token }),
                    canonical(ctx, at, *factory, isB20InitializedCall { token }),
                    canonical(ctx, at, token, isPausedCall { feature: 0 })
                )?;
                if code.as_ref() != [0xef]
                    || keccak256(&code) != t.runtime
                    || !exists
                    || !ready
                    || paused
                {
                    return Err(bad());
                }
                for (name, account) in [
                    ("TRANSFER_SENDER_POLICY", from),
                    ("TRANSFER_RECEIVER_POLICY", to),
                    ("TRANSFER_EXECUTOR_POLICY", executor),
                ] {
                    let id = canonical(
                        ctx,
                        at,
                        token,
                        policyIdCall {
                            scope: keccak256(name.as_bytes()),
                        },
                    )
                    .await?;
                    let (set, ok) = tokio::try_join!(
                        canonical(ctx, at, *registry, policyExistsCall { id }),
                        canonical(ctx, at, *registry, isAuthorizedCall { id, account })
                    )?;
                    if !set || !ok {
                        return Err(bad());
                    }
                }
                Ok(())
            }
        }
    }
}
