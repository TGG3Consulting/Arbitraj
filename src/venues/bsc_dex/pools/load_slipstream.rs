//! Файл: venues/bsc_dex/pools/load_slipstream.rs
//! Что делает: чтение состояния пула Slipstream (Topaz v3 на BSC, решения 80, 85) на заданном блоке: slot0 (цена, тик),
//! liquidity и инициализированные тики по битмапу в словах вокруг текущего тика. Slipstream — построчный форк Uniswap
//! V3 core, поэтому математика свопа и карта тиков — те же, что у V3 (PoolState::V3 + TickMap, math/swap_cl.rs). Два
//! отличия от нашего пути V3 (load_v2v3.rs), из-за которых чтение своё: (1) slot0 форка без поля feeProtocol (6 полей,
//! не 7 — свой ABI slot0, abi_slipstream.rs); (2) тики читаются ПРЯМЫМИ геттерами пула tickBitmap(word)+ticks(tick)
//! через Multicall3, а не через TickLens — у Slipstream свой TickLens с иным форматом ответа (лишнее поле sqrtRatioX96),
//! и отдельного адреса TickLens в настройках не требуется. Комиссия пула (fee_pips) берётся при проверке пула с его
//! fee() (verify.rs, НЕ из таблицы дефолтов по tickSpacing) — здесь не читается.
//! Читается по одному пулу (как Elastic/Algebra): свой набор геттеров под пачку load_batch.rs (ступени V3 завязаны на
//! TickLens) не ложится; здесь это 2 вызова головы + пачка tickBitmap + пачка ticks на пул.
//! Кто вызывает: pools/load.rs (и через него pools/load_batch.rs для Slipstream).
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_slipstream.rs, pools/load_cl.rs (set_ticks, i24),
//! load_v2v3.rs (word_range), ticks.rs.
//! Настройки: dex.tick_words, dex.depth_band_pct [bsc_dex] (сколько слов битмапа вокруг тика — решение 73, через
//! word_range).
//!
//! Геттеры — официальный Slipstream velodrome-finance/slipstream (HEAD 193a6b8, решение 23): CLPool.slot0/liquidity
//! (contracts/core/CLPool.sol#L69-L105), tickBitmap(int16)/ticks(int24) (CLPool.sol#L110-L112). Тик из бита:
//! tick = (word·256 + bit)·tickSpacing (TickBitmap.position, обратное преобразование; load_cl::set_ticks).

use std::collections::BTreeMap;

use alloy_primitives::{Address, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_slipstream::ICLPool;
use super::load_cl::{i24, set_ticks};
use super::load_v2v3::word_range;
use super::ticks::{TickData, TickMap};

/// Состояние и карта тиков пула Slipstream.
/// Принимает: адрес пула, tickSpacing (из проверки), блок-тег.
/// Ошибки: NotConfigured (dex.tick_words), BadResponse (tickSpacing ≤ 0, тик вне int24), узел, ответ не по ABI.
pub async fn load(
    ctx: &Ctx,
    pool: Address,
    spacing: i32,
    tag: &str,
) -> GatewayResult<(PoolState, TickMap)> {
    let s = call::view(ctx, pool, &ICLPool::slot0Call {}, tag).await?;
    let liquidity = call::view(ctx, pool, &ICLPool::liquidityCall {}, tag).await?;
    let tick = s.tick.as_i32();
    // Слова битмапа вокруг текущего тика (решение 73): тот же расчёт, что у V3/CL (word_range).
    let (lo, hi) = word_range(ctx, tick, spacing)?;
    let bitmap_calls: Vec<_> = (lo..=hi)
        .map(|w| ICLPool::tickBitmapCall { wordPosition: w })
        .collect();
    let words: Vec<U256> = call::view_many(ctx, pool, &bitmap_calls, tag).await?;
    let found = set_ticks(lo, &words, spacing);
    let tick_calls = found
        .iter()
        .map(|t| {
            Ok(ICLPool::ticksCall {
                tick: i24(ctx, *t)?,
            })
        })
        .collect::<GatewayResult<Vec<_>>>()?;
    let infos = call::view_many(ctx, pool, &tick_calls, tag).await?;
    let mut ticks = BTreeMap::new();
    for (t, info) in found.iter().zip(infos) {
        // Из 10 полей Tick.Info форка нужны первые два: liquidityGross и liquidityNet (прочие — stakedLiquidityNet,
        // rewardGrowthOutsideX128 и т. д. — на цену свопа не влияют).
        ticks.insert(
            *t,
            TickData {
                gross: info.liquidityGross,
                net: info.liquidityNet,
            },
        );
    }
    let state = PoolState::V3 {
        sqrt_price_x96: U256::from(s.sqrtPriceX96),
        liquidity,
        tick,
    };
    Ok((
        state,
        TickMap {
            spacing,
            ticks,
            words: (lo, hi),
        },
    ))
}
