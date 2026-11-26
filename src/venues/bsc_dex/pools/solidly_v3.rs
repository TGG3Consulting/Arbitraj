//! Solidly V3 Ethereum / Base. Состояние, изменяемая комиссия, короткие ответы ticks и встроенный quoteSwap.
use super::super::{ctx::Ctx, math::tick_math, node::call};
use super::abi_slipstream::ICLPool;
use super::load_cl::{i24, set_ticks};
use super::load_v2v3::word_range;
use super::ticks::{TickData, TickMap};
use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, I256, U256};

sol! {
    interface ISolidlyV3 {
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint24 fee, bool unlocked);
        function ticks(int24 tick) external view returns (uint128 gross, int128 net, bool initialized);
        function quoteSwap(bool zeroForOne, int256 amountSpecified, uint160 sqrtPriceLimitX96)
            external view returns (int256 amount0, int256 amount1, uint160 sqrtPriceAfter, int24 tickAfter, uint128 liquidityAfter);
        event SetFee(uint24 feeOld, uint24 feeNew);
        event PoolCreated(address indexed token0, address indexed token1, uint24 fee, int24 indexed tickSpacing, address pool);
    }
    interface ISolidlyV3Executor {
        function execute(address pool, address tokenIn, address tokenOut, uint256 amountIn, uint256 minOut, uint256 deadline) external returns (uint256);
    }
}

pub async fn load(
    ctx: &Ctx,
    pool: Address,
    spacing: i32,
    tag: &str,
) -> GatewayResult<(PoolState, TickMap, u32)> {
    let head = call::view(ctx, pool, &ISolidlyV3::slot0Call {}, tag).await?;
    let fee = head.fee.to::<u32>();
    if !head.unlocked || fee > 100_000 || spacing <= 0 {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            "Solidly V3: недопустимая комиссия, шаг тиков или пул закрыт",
        ));
    }
    let liquidity = call::view(ctx, pool, &ICLPool::liquidityCall {}, tag).await?;
    let tick = head.tick.as_i32();
    let (lo, hi) = word_range(ctx, tick, spacing)?;
    let bitmap: Vec<_> = (lo..=hi)
        .map(|word| ICLPool::tickBitmapCall { wordPosition: word })
        .collect();
    let words = call::view_many(ctx, pool, &bitmap, tag).await?;
    let indexes = set_ticks(lo, &words, spacing);
    let calls = indexes
        .iter()
        .map(|t| {
            Ok(ISolidlyV3::ticksCall {
                tick: i24(ctx, *t)?,
            })
        })
        .collect::<GatewayResult<Vec<_>>>()?;
    let info = call::view_many(ctx, pool, &calls, tag).await?;
    let mut ticks = std::collections::BTreeMap::new();
    for (index, row) in indexes.into_iter().zip(info) {
        if !row.initialized || row.gross == 0 {
            return Err(ctx.err(
                ErrorCode::BadResponse,
                "Solidly V3: установленный бит у пустого тика",
            ));
        }
        ticks.insert(
            index,
            TickData {
                gross: row.gross,
                net: row.net,
            },
        );
    }
    Ok((
        PoolState::V3 {
            sqrt_price_x96: U256::from(head.sqrtPriceX96),
            tick,
            liquidity,
        },
        TickMap {
            spacing,
            words: (lo, hi),
            ticks: ticks.into(),
        },
        fee,
    ))
}

pub fn quote_data(zero_for_one: bool, amount: U256) -> Option<Bytes> {
    let amount = I256::try_from(amount).ok()?;
    if !amount.is_positive() {
        return None;
    }
    let limit = if zero_for_one {
        tick_math::min_sqrt_ratio() + U256::ONE
    } else {
        tick_math::max_sqrt_ratio() - U256::ONE
    };
    Some(
        ISolidlyV3::quoteSwapCall {
            zeroForOne: zero_for_one,
            amountSpecified: amount,
            sqrtPriceLimitX96: limit.to(),
        }
        .abi_encode()
        .into(),
    )
}

/// Выход учитывается только при полном списании исходного количества.
pub fn quote_output(raw: &[u8], zero_for_one: bool, expected_in: U256) -> Option<U256> {
    let q = ISolidlyV3::quoteSwapCall::abi_decode_returns_validate(raw).ok()?;
    let (used, out) = if zero_for_one {
        (q.amount0, q.amount1)
    } else {
        (q.amount1, q.amount0)
    };
    (used.is_positive() && used.into_raw() == expected_in && out.is_negative())
        .then(|| out.unsigned_abs())
}

pub fn execution_data(
    pool: Address,
    input: Address,
    output: Address,
    amount: U256,
    minimum: U256,
    deadline: U256,
) -> Bytes {
    ISolidlyV3Executor::executeCall {
        pool,
        tokenIn: input,
        tokenOut: output,
        amountIn: amount,
        minOut: minimum,
        deadline,
    }
    .abi_encode()
    .into()
}

/// Хеш попадает в справочник выпуска после канонических симуляций собранного модуля на этой сети.
/// Проверенная сборка и адрес, заданный владельцем, сверяются независимо.
pub fn accepted_runtime(chain: u64) -> Option<alloy_primitives::B256> {
    super::direct_module::accepted(chain, "solidly_v3")
}

pub async fn verify_executor(ctx: &Ctx, router: Address, tag: &str) -> GatewayResult<()> {
    super::direct_module::verify_code(ctx, "solidly_v3", router, &serde_json::json!(tag)).await
}
