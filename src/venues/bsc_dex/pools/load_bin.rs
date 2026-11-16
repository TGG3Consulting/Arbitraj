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
//! Ревью 332 (Н7): обход полосы не держит сессию пачки вдвое-вчетверо дольше нужного — стороны вниз и вверх читаются
//! одновременно (side), а на каждом шаге стороны getBin(бин) и getNextNonEmptyBin(от этого бина) уходят вместе (оба
//! зависят только от id бина): один круг к узлу на бин вместо двух подряд. Итог тот же, что у последовательного обхода
//! (те же вызовы на том же блоке); лишний вызов — один getNextNonEmptyBin на сторону, где обход остановился по
//! dex.bin_depth_usd. Края «дальше бинов нет» — bins.rs (NONE_BELOW / NONE_ABOVE, Н6).

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
use super::bins::{BinMap, NONE_ABOVE, NONE_BELOW};

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
    let at = Walk {
        ctx,
        manager,
        id,
        step,
        depth,
        limit,
        tag,
    };
    // Ревью 332 (Н7): активный бин и обе стороны — одновременно.
    let ((x, y), (below, low), (above, high)) = futures_util::future::try_join3(
        bin(ctx, manager, id, active, tag),
        side(&at, active, true),
        side(&at, active, false),
    )
    .await?;
    let mut reserves: BTreeMap<u32, (u128, u128)> = below.into_iter().chain(above).collect();
    if x != 0 || y != 0 {
        reserves.insert(active, (x, y));
    }
    // Сторона до конца пула — край NONE_ABOVE внизу / NONE_BELOW вверху (bins.rs); иначе — последний прочитанный бин.
    let range = (low.unwrap_or(NONE_ABOVE), high.unwrap_or(NONE_BELOW));
    let state = PoolState::Bin {
        active_id: active,
        protocol_fee: s.protocolFee.to(),
        lp_fee: s.lpFee.to(),
    };
    Ok((
        state,
        BinMap {
            step,
            reserves: reserves.into(),
            range,
        },
    ))
}

/// Сторона полосы: прочитанные бины (id, (X, Y)) и край (None — сторона до конца пула).
type Side = (Vec<(u32, (u128, u128))>, Option<u32>);

/// Что нужно обходу стороны полосы (см. load).
struct Walk<'a> {
    ctx: &'a Ctx,
    manager: alloy_primitives::Address,
    id: B256,
    step: u32,
    depth: &'a Depth,
    limit: Decimal,
    tag: &'a str,
}

/// Следующий непустой бин строго ниже (down) / выше от `from` (getNextNonEmptyBin). Ошибки: узел, ответ не по ABI.
async fn next_bin(w: &Walk<'_>, from: u32, down: bool) -> GatewayResult<u32> {
    let c = IInfinityBin::getNextNonEmptyBinCall {
        id: w.id,
        swapForY: down,
        binId: U24::from(from),
    };
    Ok(call::view(w.ctx, w.manager, &c, w.tag).await?.to())
}

/// Одна сторона полосы от активного бина: непустые бины по порядку, пока их стоимость не достигнет dex.bin_depth_usd
/// (или бины не кончатся). Принимает: обход, активный бин, сторону (true — вниз). Возвращает: (бины с резервами,
/// край: Some — последний прочитанный бин, остановка по стоимости; None — дальше бинов нет, сторона до конца пула).
/// Ошибки: узел, ответ не по ABI, стоимость бина не считается (usdt_value).
async fn side(w: &Walk<'_>, active: u32, down: bool) -> GatewayResult<Side> {
    let mut out = Vec::new();
    let mut value = Decimal::ZERO;
    let mut next = next_bin(w, active, down).await?;
    loop {
        if (down && next == NONE_BELOW) || (!down && next == NONE_ABOVE) {
            return Ok((out, None));
        }
        // Н7: резервы бина и следующий непустой за ним — одним кругом (оба зависят только от id).
        let (b, after) = futures_util::future::try_join(
            bin(w.ctx, w.manager, w.id, next, w.tag),
            next_bin(w, next, down),
        )
        .await?;
        out.push((next, b));
        value += usdt_value(w.ctx, b.0, b.1, next, w.step, w.depth)?;
        if value >= w.limit {
            return Ok((out, Some(next)));
        }
        next = after;
    }
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
pub fn usdt_value(
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
