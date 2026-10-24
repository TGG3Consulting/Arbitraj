//! Файл: venues/bsc_dex/pools/load_bin.rs
//! Что делает: чтение состояния пула Infinity Bin на заданном блоке: slot0 (активный бин, комиссии) и резервы
//! непустых бинов в обе стороны от активного, пока стоимость прочитанных бинов в каждой стороне не достигнет
//! dex.bin_depth_usd (или бины не кончатся). Плюс перечитывание отдельных бинов после событий.
//! Кто вызывает: pools/load.rs, pools/events_bin.rs.
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_v4.rs, bins.rs, math/bin_price.rs, tokens.rs, num.rs.
//! Настройки: dex.bin_depth_usd [bsc_dex].
//!
//! getSlot0, getBin, getNextNonEmptyBin — IBinPoolManager.sol#L137-L167 (infinity-core 3c5ef1be).
//! Направление поиска — по коду, не по комментарию интерфейса: getNextNonEmptyBin(swapForY = true) вызывает
//! TreeMath.findFirstRight — первый непустой бин СТРОГО НИЖЕ id, нет такого — type(uint24).max;
//! swapForY = false → findFirstLeft — строго выше, нет — 0 (src/pool-bin/libraries/BinPool.sol#L293-L297,
//! src/pool-bin/libraries/math/TreeMath.sol#L83-L215). Комментарий в IBinPoolManager.sol#L161-L166 говорит обратное.
//! Стоимость бина в USDT: X = currency0, Y = currency1, цена 128.128 = Y за 1 X (PriceHelper).

use std::collections::BTreeMap;

use alloy_primitives::aliases::U24;
use alloy_primitives::{B256, U256};
use rust_decimal::Decimal;

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::bin_price::{SCALE_OFFSET, price_from_id};
use super::super::node::call;
use super::super::num;
use super::abi_v4::IInfinityBin;
use super::bins::BinMap;

/// Конец поиска вниз (findFirstRight): type(uint24).max.
const NONE_BELOW: u32 = 0xff_ffff;
/// Конец поиска вверх (findFirstLeft): 0.
const NONE_ABOVE: u32 = 0;

/// Что нужно для оценки стоимости бина в USDT.
pub struct Depth {
    /// USDT — это Y (currency1); иначе X.
    pub usdt_is_y: bool,
    /// decimals USDT.
    pub usdt_decimals: u8,
}

/// Состояние и резервы бинов.
/// Принимает: менеджер, PoolId, binStep, оценку стоимости, блок.
/// Ошибки: NotConfigured (dex.bin_depth_usd), узел, ответ не по ABI.
pub async fn load(
    ctx: &Ctx,
    manager: alloy_primitives::Address,
    id: B256,
    step: u32,
    depth: &Depth,
    tag: &str,
) -> GatewayResult<(PoolState, BinMap)> {
    let limit = ctx.dec("dex.bin_depth_usd")?;
    let s = call::view(ctx, manager, &IInfinityBin::getSlot0Call { id }, tag).await?;
    let active: u32 = s.activeId.to();
    let mut reserves = BTreeMap::new();
    let (x, y) = bin(ctx, manager, id, active, tag).await?;
    if x != 0 || y != 0 {
        reserves.insert(active, (x, y));
    }
    let mut range = (active, active);
    for down in [true, false] {
        let mut cur = active;
        let mut value = Decimal::ZERO;
        loop {
            let c = IInfinityBin::getNextNonEmptyBinCall {
                id,
                swapForY: down,
                binId: U24::from(cur),
            };
            let next: u32 = call::view(ctx, manager, &c, tag).await?.to();
            if (down && next == NONE_BELOW) || (!down && next == NONE_ABOVE) {
                // Дальше бинов нет: вся сторона известна (пустая).
                if down {
                    range.0 = 0
                } else {
                    range.1 = NONE_BELOW
                }
                break;
            }
            let (x, y) = bin(ctx, manager, id, next, tag).await?;
            reserves.insert(next, (x, y));
            if down {
                range.0 = next
            } else {
                range.1 = next
            }
            value += usdt_value(ctx, x, y, next, step, depth)?;
            if value >= limit {
                break;
            }
            cur = next;
        }
    }
    let state = PoolState::Bin {
        active_id: active,
        protocol_fee: s.protocolFee.to(),
        lp_fee: s.lpFee.to(),
    };
    Ok((
        state,
        BinMap {
            step,
            reserves,
            range,
        },
    ))
}

/// Резервы одного бина. Ошибки: узел, ответ не по ABI.
pub async fn bin(
    ctx: &Ctx,
    manager: alloy_primitives::Address,
    id: B256,
    bin_id: u32,
    tag: &str,
) -> GatewayResult<(u128, u128)> {
    let r = call::view(
        ctx,
        manager,
        &IInfinityBin::getBinCall {
            id,
            binId: U24::from(bin_id),
        },
        tag,
    )
    .await?;
    Ok((r.binReserveX, r.binReserveY))
}

/// Стоимость бина в USDT: резерв USDT + резерв монеты по цене бина.
/// Ошибки: BadResponse — цена не считается (как откат контракта) или сумма не помещается в Decimal.
fn usdt_value(
    ctx: &Ctx,
    x: u128,
    y: u128,
    bin_id: u32,
    step: u32,
    d: &Depth,
) -> GatewayResult<Decimal> {
    let price = price_from_id(bin_id, step).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Цена бина {bin_id} не считается (PowUnderflow)"),
        )
    })?;
    let units = if d.usdt_is_y {
        U256::from(y).saturating_add(U256::from(x).saturating_mul(price) >> SCALE_OFFSET)
    } else {
        U256::from(x).saturating_add((U256::from(y) << SCALE_OFFSET) / price)
    };
    num::to_decimal(units, d.usdt_decimals).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Стоимость бина {bin_id} не помещается в Decimal"),
        )
    })
}
