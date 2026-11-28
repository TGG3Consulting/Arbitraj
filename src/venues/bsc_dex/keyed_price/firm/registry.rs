//! 0x публикует текущий и предыдущий Settler для функции 2 в реестре размещений.
//! https://github.com/0xProject/0x-settler
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, node::call, pools::solidly_classic_factory},
};
use alloy_primitives::{Address, B256, U256};
use serde_json::Value;
alloy::sol! { interface IRegistry {function ownerOf(uint256 id) external view returns(address);function prev(uint128 feature) external view returns(address);}}
/// 0x предписывает сначала подтвердить активного владельца feature NFT. При первом
/// размещении чтение предыдущего экземпляра действительно откатывается на Base.
async fn registered<E>(
    expected: Address,
    owner: impl std::future::Future<Output = Result<Address, E>>,
    previous: impl std::future::Future<Output = Result<Address, E>>,
) -> Result<bool, E> {
    let active = owner.await?;
    if expected.is_zero() || active.is_zero() {
        return Ok(false);
    }
    Ok(active == expected || previous.await? == expected)
}

pub(in crate::venues::bsc_dex) async fn verify(ctx: &Ctx, settler: Address, at: &Value) -> GatewayResult<()> {
    let fail = || super::bad(ctx, "Реестр 0x не подтвердил действующий Settler");
    let data = ctx.text("zeroex.settler_registry")?;
    let fields = data.split_whitespace().collect::<Vec<_>>();
    let [proxy, shell, logic, code] = fields.as_slice() else {
        return Err(fail());
    };
    let address = |s: &str| {
        s.parse::<Address>()
            .ok()
            .filter(|x| !x.is_zero())
            .ok_or_else(fail)
    };
    let hash = |s: &str| {
        s.parse::<B256>()
            .ok()
            .filter(|x| !x.is_zero())
            .ok_or_else(fail)
    };
    let registry = address(proxy)?;
    let ((), accepted) = tokio::try_join!(
        solidly_classic_factory::bind(
            ctx,
            registry,
            hash(shell)?,
            Some((address(logic)?, hash(code)?)),
            at
        ),
        registered(
            settler,
            call::canonical(
                ctx,
                at,
                registry,
                IRegistry::ownerOfCall { id: U256::from(2) }
            ),
            call::canonical(ctx, at, registry, IRegistry::prevCall { feature: 2 })
        )
    )?;
    if !accepted {
        return Err(fail());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    fn parsed(v: &Value) -> Result<Address, ()> {
        let hex = v["response"]["result"].as_str().ok_or(())?;
        let value = hex.parse::<B256>().map_err(|_| ())?;
        Ok(Address::from_slice(&value[12..]))
    }

    #[tokio::test]
    async fn first_settler_works_while_previous_reverts_and_dwell_uses_real_previous() {
        let raw: Value = serde_json::from_str(include_str!(
            "../../../../../tests/data/first_run_dex_2026-11-26/zeroex_base_registry_roles.json"
        ))
        .unwrap();
        let rows = raw["observations"].as_array().unwrap();
        assert_eq!(raw["feature"], 2);
        assert_eq!(rows[0]["block"], 14769149);
        assert!(rows[0]["previous"]["response"]["error"]["code"].is_number());
        for r in rows {
            let owner = parsed(&r["current"]).unwrap();
            let previous = parsed(&r["previous"]);
            for field in ["current", "previous"] {
                assert_eq!(r[field]["request"]["params"][1], r["selector"]);
                assert_eq!(r["selector"]["requireCanonical"], true);
            }
            let reads = Cell::new(0);
            let prev = || async {
                reads.set(reads.get() + 1);
                previous
            };
            assert_eq!(
                registered(owner, async { Ok(owner) }, prev()).await,
                Ok(true)
            );
            assert_eq!(reads.get(), 0);
            if previous.is_err() {
                let foreign: Address = raw["registry"].as_str().unwrap().parse().unwrap();
                assert_eq!(registered(foreign, async { Ok(owner) }, prev()).await, Err(()));
                assert_eq!(reads.get(), 1);
            }
            if let Ok(old) = previous {
                assert_ne!(old, owner);
                assert_eq!(registered(old, async { Ok(owner) }, prev()).await, Ok(true));
                assert_eq!(reads.get(), 1);
                let registry: Address = raw["registry"].as_str().unwrap().parse().unwrap();
                assert_eq!(
                    registered(registry, async { Ok(owner) }, prev()).await,
                    Ok(false)
                );
                assert_eq!(registered(old, async { Err(()) }, prev()).await, Err(()));
                assert_eq!(reads.get(), 2);
            }
        }
    }
}

#[cfg(test)]
#[path = "zeroex_base_tests.rs"]
mod cash_tests;
