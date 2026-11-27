//! Канонический PoolKey штатного хука EulerSwap и расчёт через существующий PoolManager V4.
use super::{
    families::Family,
    key::{PoolKey, UniswapPoolKeyData},
    registry::Entry,
};
use crate::{
    core::types::{PoolRef, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, exec::executor::kind_key::V4Key, node::call},
};
use alloy_primitives::Address;
use serde_json::Value;

alloy::sol! {
    struct HookKey {address currency0;address currency1;uint24 fee;int24 tickSpacing;address hooks;}
    interface IEulerHook {
        function poolKey() external view returns(HookKey);
        function poolManager() external view returns(address);
    }
}

pub(crate) async fn canonical(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<PoolKey> {
    let expected = ctx.dex_addr("dex.pool_manager", f.id)?;
    let (k, manager) = tokio::try_join!(
        call::canonical(ctx, at, pool, IEulerHook::poolKeyCall {}),
        call::canonical(ctx, at, pool, IEulerHook::poolManagerCall {})
    )?;
    if manager.is_zero()
        || manager != expected
        || (k.currency0, k.currency1) != pair
        || k.hooks != pool
        || k.tickSpacing.as_i32() <= 0
    {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "EulerSwap: менеджер или ключ хука не соответствует подтверждённому пулу",
        ));
    }
    Ok(PoolKey::Uniswap(UniswapPoolKeyData {
        currency0: k.currency0,
        currency1: k.currency1,
        fee: k.fee.to(),
        tick_spacing: k.tickSpacing,
        hooks: k.hooks,
    }))
}

pub(crate) fn key(e: &Entry) -> Option<V4Key> {
    let PoolRef::Contract(pool) = e.pool.reference else {
        return None;
    };
    let Some(PoolKey::Uniswap(k)) = &e.key else {
        return None;
    };
    (matches!(e.pool.state, PoolState::OraclePmm(_))
        && k.hooks == pool
        && !pool.is_zero()
        && (k.currency0, k.currency1) == (e.pool.token0, e.pool.token1)
        && k.tick_spacing.as_i32() > 0)
        .then(|| V4Key::of(k))
}

pub(crate) fn step(ctx: &Ctx, e: &Entry) -> Option<(V4Key, Address)> {
    Some((
        key(e)?,
        ctx.dex_addr("dex.pool_manager", e.family.id)
            .ok()
            .filter(|m| !m.is_zero())?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::bsc_dex::exec::executor::abi::ArbExecutor;
    use alloy::sol_types::{SolCall, SolValue};
    use alloy_primitives::{Bytes, U256};
    use flate2::read::GzDecoder;
    use serde_json::Value;

    #[test]
    fn original_manager_pays_for_both_generations_with_the_signed_hook() {
        let samples: [(&[u8], usize, usize); 2] = [
            (
                include_bytes!(
                    "../../../../tests/data/first_run_dex_2026-11-26/eulerswap_ethereum_v4.json.gz"
                ),
                6,
                6,
            ),
            (
                include_bytes!(
                    "../../../../tests/data/first_run_dex_2026-11-26/eulerswap_base_v4.json.gz"
                ),
                4,
                4,
            ),
        ];
        for (data, accepted, refused) in samples {
            let sample: Value = serde_json::from_reader(GzDecoder::new(data)).unwrap();
            let yes = sample["cases"].as_array().unwrap();
            assert_eq!(yes.len(), accepted);
            for row in yes {
                let payload: Bytes = row["order"]["data"].as_str().unwrap().parse().unwrap();
                let order = ArbExecutor::executeCall::abi_decode(&payload).unwrap();
                let [step] = order.steps.as_slice() else {
                    panic!("Ожидался один шаг EulerSwap");
                };
                let key = HookKey::abi_decode(&step.data).unwrap();
                let original = V4Key {
                    currency0: key.currency0,
                    currency1: key.currency1,
                    fee: key.fee.to(),
                    tick_spacing: key.tickSpacing,
                    hooks: key.hooks,
                };
                assert_eq!(original.pool_key().abi(), step.data.as_ref());
                assert_eq!(
                    key.hooks,
                    row["pool"].as_str().unwrap().parse::<Address>().unwrap()
                );
                assert_eq!(step.kind, 19);
                assert_ne!(step.pool, key.hooks);
                assert_eq!(order.amountIn, U256::from(row["paid"].as_u64().unwrap()));
                assert_eq!(order.minOut, U256::from(row["net"].as_u64().unwrap()));
                assert!(row["strict"].is_object() && row["expired"].is_object());
            }
            let no = sample["refusals"].as_array().unwrap();
            assert_eq!(no.len(), refused);
            assert!(no.iter().all(|row| {
                row["before"] == row["after"]
                    && row["error"]["data"]
                        .as_str()
                        .is_some_and(|e| e.contains("34373fbc"))
            }));
        }
    }
}
