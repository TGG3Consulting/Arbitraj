//! Файл: venues/bsc_dex/pools/load_elastic.rs
//! Что делает: чтение состояния пула KyberSwap Elastic (решение 81) на заданном блоке: getPoolState (цена, тик),
//! getLiquidityState (baseL, reinvestL), swapFeeUnits (комиссия), и карта тиков — все инициализированные тики
//! связного списка через читатель TicksFeesReader.getTicksInRange + liquidityNet каждого тика методом пула ticks().
//! Читается по одному пулу (как Bin): обход списка тиков у Elastic — не битмап, под пачку load_batch.rs он не ложится
//! (там ступени по словам битмапа); здесь это один вызов читателя + одна пачка ticks() на пул.
//! Кто вызывает: pools/load.rs (и через него pools/load_batch.rs для Elastic).
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_elastic.rs, pools/load_cl.rs (i24), ticks.rs, math/tick_math.rs.
//! Настройки: dex.tick_reader [DEX] (адрес TicksFeesReader).
//!
//! Геттеры и читатель — ks-elastic-sc (решение 23): IPoolStorage.getPoolState/getLiquidityState/swapFeeUnits/ticks
//! (#L28-L101), TicksFeesReader.getTicksInRange(pool, MIN_TICK, 0) → все инициализированные тики вверх по списку
//! (#L37-L67; длина 0 = максимум). В карту берём тики с ненулевой liquidityGross (как у V3/CL), прочие (в т.ч.
//! граничные MIN_TICK/MAX_TICK списка) опускаем. words карты — весь диапазон: Elastic обновляет карту не по словам
//! (события Mint/Burn → перечитывание целиком, events_elastic.rs), а covers()/modify() для него не вызываются.

use std::collections::BTreeMap;

use alloy_primitives::{Address, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::math::tick_math::{MAX_TICK, MIN_TICK};
use super::super::node::call;
use super::abi_elastic::{IElasticPool, ITicksFeesReader};
use super::families::Family;
use super::load_cl::i24;
use super::ticks::{TickData, TickMap};

/// Состояние и карта тиков пула Elastic.
/// Принимает: вид, адрес пула, tickDistance (из проверки), блок-тег.
/// Ошибки: NotConfigured (dex.tick_reader), узел, ответ не по ABI, тик вне int24.
pub async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    spacing: i32,
    tag: &str,
) -> GatewayResult<(PoolState, TickMap)> {
    let reader = ctx.dex_addr("dex.tick_reader", f.id)?;
    let st = call::view(ctx, pool, &IElasticPool::getPoolStateCall {}, tag).await?;
    let liq = call::view(ctx, pool, &IElasticPool::getLiquidityStateCall {}, tag).await?;
    let fee = call::view(ctx, pool, &IElasticPool::swapFeeUnitsCall {}, tag).await?;
    let tick = st.currentTick.as_i32();
    let state = PoolState::Elastic {
        sqrt_price_x96: U256::from(st.sqrtP),
        tick,
        base_l: liq.baseL,
        reinvest_l: liq.reinvestL,
        fee_units: fee.to(),
    };
    // Все инициализированные тики списка (вверх от MIN_TICK) одним вызовом читателя.
    let all = call::view(
        ctx,
        reader,
        &ITicksFeesReader::getTicksInRangeCall {
            pool,
            startTick: i24(ctx, MIN_TICK)?,
            length: 0,
        },
        tag,
    )
    .await?;
    let found = parse_ticks(&all);
    // liquidityNet каждого найденного тика.
    let calls: Vec<_> = found
        .iter()
        .map(|t| {
            Ok(IElasticPool::ticksCall {
                tick: i24(ctx, *t)?,
            })
        })
        .collect::<GatewayResult<Vec<_>>>()?;
    let infos = call::view_many(ctx, pool, &calls, tag).await?;
    let mut map: BTreeMap<i32, TickData> = BTreeMap::new();
    for (t, i) in found.iter().zip(infos) {
        if i.liquidityGross > 0 {
            map.insert(
                *t,
                TickData {
                    gross: i.liquidityGross,
                    net: i.liquidityNet,
                },
            );
        }
    }
    Ok((
        state,
        TickMap {
            spacing,
            ticks: map,
            words: (i16::MIN, i16::MAX),
        },
    ))
}

/// Тики из ответа getTicksInRange: массив заранее задан максимальной длиной и после MAX_TICK добит нулями
/// (TicksFeesReader.sol) — берём по MAX_TICK включительно и останавливаемся. Ошибок нет.
pub fn parse_ticks(all: &[alloy_primitives::aliases::I24]) -> Vec<i32> {
    let mut out = Vec::new();
    for t in all {
        let t = t.as_i32();
        out.push(t);
        if t >= MAX_TICK {
            break;
        }
    }
    out
}
