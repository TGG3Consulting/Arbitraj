//! Файл: venues/bsc_dex/pools/load_ramses.rs
//! Что делает: решение 483 — чтение состояния пула Ramses обеих CL-линий на заданном блоке (варианты RamsesCl и
//! RamsesClV2, pools/family_variant.rs): slot0 (цена, тик), liquidity, слова tickBitmap вокруг тика и ticks() тиков из
//! битмапа — прямыми геттерами пула (как load_slipstream.rs), плюс комиссия обмена на том же блоке. Отличия от
//! load_slipstream.rs — только ABI ответов и комиссия:
//! - RamsesCl (RamsesV2Pool): slot0 из 7 полей с uint8 feeProtocol (L239-L263), ticks() из 10 полей (L338-L370;
//!   liquidityNet — второе, boostedLiquidity* на обмен не влияют: цикл swap берёт state.liquidity L1053 и liquidityNet
//!   из Tick.cross L1124-L1150), комиссия — currentFee() (L216-L218 = states.fee в computeSwapStep L1044-L1056;
//!   fee() — начальная, L199-L201). TickLens Uniswap разбирает ticks() как 8 полей Uni V3 — на ответе из 10 полей
//!   он не проверен записью, поэтому тики — прямыми геттерами пула.
//! - RamsesClV2 (RamsesV3Pool): slot0 из 7 полей с uint24 feeProtocol (L817-L842), ticks() из 8 полей (L896-L920),
//!   комиссия — fee() (L798-L800 = $.fee в computeSwapStep L504-L512).
//!
//! Комиссия читается при каждом чтении пула (её меняет setFee без пересоздания пула — ProtocolActions.sol; событие
//! FeeAdjustment — ускорение между чтениями, pools/ramses.rs): сделка всегда считается по комиссии блока верификации.
//! liquidity() и tickBitmap(int16) — те же подписи, что у Slipstream (ICLPool, сверено тестом abi_ramses.rs).
//! Кто вызывает: pools/load.rs (state).
//! От чего зависит: ctx.rs, node/call.rs, abi_ramses.rs, abi_slipstream.rs (ICLPool), load_cl.rs (i24, set_ticks),
//! load_v2v3.rs (word_range), ticks.rs, families.rs, family_variant.rs.
//! Настройки: dex.tick_words [bsc_dex] (через word_range).

use std::collections::BTreeMap;

use alloy_primitives::aliases::I24;
use alloy_primitives::{Address, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_ramses::{IRamsesClPool, IRamsesV3Pool};
use super::abi_slipstream::ICLPool;
use super::families::Family;
use super::family_variant::Variant;
use super::load_cl::{i24, set_ticks};
use super::load_v2v3::word_range;
use super::ticks::{TickData, TickMap};

/// Состояние, карта тиков и комиссия (пипсы) пула Ramses CL / CL V2. Принимает: окружение, вид, адрес пула,
/// tickSpacing (из проверки), блок-тег. Ошибки: Internal (вид не Ramses CL), NotConfigured (dex.tick_words),
/// BadResponse (тик вне int24), узел, ответ не по ABI.
pub async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    spacing: i32,
    tag: &str,
) -> GatewayResult<(PoolState, TickMap, u32)> {
    let v2line = match f.variant {
        Variant::RamsesCl => true,
        Variant::RamsesClV2 | Variant::RegisteredSpacingV3 => false,
        _ => {
            let why = format!("{}: чтение Ramses для вида не Ramses CL", f.id);
            return Err(ctx.err(ErrorCode::Internal, why));
        }
    };
    let (sqrt, tick) = if f.variant.spacing_keyed() {
        let s=call::view(ctx,pool,&super::abi_v2v3::IUniswapV3Pool::slot0Call{},tag).await?;
        (U256::from(s.sqrtPriceX96),s.tick.as_i32())
    } else if v2line {
        let s = call::view(ctx, pool, &IRamsesClPool::slot0Call {}, tag).await?;
        (U256::from(s.sqrtPriceX96), s.tick.as_i32())
    } else {
        let s = call::view(ctx, pool, &IRamsesV3Pool::slot0Call {}, tag).await?;
        (U256::from(s.sqrtPriceX96), s.tick.as_i32())
    };
    let liquidity = call::view(ctx, pool, &ICLPool::liquidityCall {}, tag).await?;
    let fee = if v2line {
        call::view(ctx, pool, &IRamsesClPool::currentFeeCall {}, tag).await?
    } else {
        call::view(ctx, pool, &IRamsesV3Pool::feeCall {}, tag).await?
    };
    // Слова битмапа вокруг текущего тика (решение 73) — тот же расчёт, что у V3 / Slipstream.
    let (lo, hi) = word_range(ctx, tick, spacing)?;
    let bitmap_calls: Vec<_> = (lo..=hi)
        .map(|w| ICLPool::tickBitmapCall { wordPosition: w })
        .collect();
    let words: Vec<U256> = call::view_many(ctx, pool, &bitmap_calls, tag).await?;
    let found = set_ticks(lo, &words, spacing);
    let nets = if v2line {
        let calls = tick_calls(ctx, &found, |tick| IRamsesClPool::ticksCall { tick })?;
        let got = call::view_many(ctx, pool, &calls, tag).await?;
        got.into_iter()
            .map(|t| (t.liquidityGross, t.liquidityNet))
            .collect::<Vec<_>>()
    } else {
        let calls = tick_calls(ctx, &found, |tick| IRamsesV3Pool::ticksCall { tick })?;
        let got = call::view_many(ctx, pool, &calls, tag).await?;
        got.into_iter()
            .map(|t| (t.liquidityGross, t.liquidityNet))
            .collect::<Vec<_>>()
    };
    let mut ticks = BTreeMap::new();
    for (t, (gross, net)) in found.iter().zip(nets) {
        ticks.insert(*t, TickData { gross, net });
    }
    let state = PoolState::V3 {
        sqrt_price_x96: sqrt,
        liquidity,
        tick,
    };
    let map = TickMap {
        spacing,
        ticks: ticks.into(),
        words: (lo, hi),
    };
    Ok((state, map, fee.to::<u32>()))
}

/// Вызовы ticks() установленных тиков. Принимает: окружение, тики, конструктор вызова. Ошибка BadResponse — тик вне
/// int24.
fn tick_calls<C>(ctx: &Ctx, found: &[i32], make: impl Fn(I24) -> C) -> GatewayResult<Vec<C>> {
    found.iter().map(|t| Ok(make(i24(ctx, *t)?))).collect()
}
