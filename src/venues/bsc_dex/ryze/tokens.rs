//! Точный перевод WETH9 и FiatToken со слотом ZeppelinOS. В одном блоке проверяет код, администратор, реализацию и запреты переводов.
use super::super::{
    ctx::Ctx,
    node::{call::canonical, rpc},
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::primitives::{Address, B256, Bytes, keccak256};
use serde_json::{Value, json};
alloy::sol! {
    function paused() external view returns(bool);
    function isBlacklisted(address account) external view returns(bool);
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Token {
    pub address: Address,
    pub runtime: B256,
    pub proxy: Option<(Address, B256, Address)>,
}
impl Token {
    pub fn parse(s: &str) -> Option<Self> {
        let f: Vec<_> = s.split('/').map(str::trim).collect();
        let (address, runtime, proxy) = match f.as_slice() {
            [a, h] => (a.parse().ok()?, h.parse().ok()?, None),
            [a, h, i, x, admin] => (
                a.parse().ok()?,
                h.parse().ok()?,
                Some((i.parse().ok()?, x.parse().ok()?, admin.parse().ok()?)),
            ),
            _ => return None,
        };
        let t = Self {
            address,
            runtime,
            proxy,
        };
        (!t.address.is_zero()
            && !t.runtime.is_zero()
            && t.proxy.is_none_or(|(i, h, a)| {
                !i.is_zero() && !h.is_zero() && !a.is_zero() && i != a && i != t.address
            }))
        .then_some(t)
    }
    pub async fn verify(
        &self,
        ctx: &Ctx,
        at: &Value,
        participants: &[Address],
    ) -> GatewayResult<()> {
        let bad = || {
            ctx.err(
                ErrorCode::NotApplicable,
                "Ryze: изменился код токена или переводы приостановлены",
            )
        };
        let code = |a: Address| rpc::call::<Bytes>(ctx, "eth_getCode", json!([a, at]));
        if keccak256(code(self.address).await?) != self.runtime {
            return Err(bad());
        }
        if let Some((implementation, hash, admin)) = self.proxy {
            let slot = |name: &str| {
                rpc::call::<B256>(
                    ctx,
                    "eth_getStorageAt",
                    json!([
                        self.address,
                        keccak256(format!("org.zeppelinos.proxy.{name}").as_bytes()),
                        at
                    ]),
                )
            };
            let (i, a, bytes, stopped) = tokio::try_join!(
                slot("implementation"),
                slot("admin"),
                code(implementation),
                canonical(ctx, at, self.address, pausedCall {})
            )?;
            if (i, a) != (implementation.into_word(), admin.into_word())
                || keccak256(bytes) != hash
                || (stopped && !participants.is_empty())
            {
                return Err(bad());
            }
            for &account in participants {
                if account == admin
                    || canonical(ctx, at, self.address, isBlacklistedCall { account }).await?
                {
                    return Err(bad());
                }
            }
        }
        Ok(())
    }
}

use super::{deployment::Config, wire};
pub(super) async fn physical(
    ctx: &super::super::ctx::Ctx,
    c: &Config,
    at: &serde_json::Value,
    path: &[wire::Hop],
    owner: Address,
) -> bool {
    let Ok(collector) = canonical(ctx, at, c.router, wire::feeCollectorCall {}).await else {
        return false;
    };
    let mut participants = vec![owner, c.router, collector];
    participants.extend(path.iter().map(|h| h.pool));
    participants.sort_unstable();
    participants.dedup();
    let tokens: std::collections::BTreeSet<_> =
        path.iter().flat_map(|h| [h.tokenIn, h.tokenOut]).collect();
    for a in tokens {
        let Some(t) = c.tokens.iter().find(|t| t.address == a) else {
            return false;
        };
        if t.verify(ctx, at, &participants).await.is_err() {
            return false;
        }
    }
    true
}
