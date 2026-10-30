//! Файл: venues/bsc_dex/pools/load_algebra.rs
//! Что делает: чтение состояния пула Algebra (THENA Fusion и форки Algebra V1/V1.9, решения 80, 81) на заданном блоке:
//! globalState (цена, тик, текущая динамическая комиссия), liquidity (одна ликвидность — reinvestL у Algebra нет), и
//! карта тиков по СЫРОМУ битмапу tickTable(int16) в словах вокруг текущего тика + liquidityDelta каждого тика методом
//! пула ticks(). Собирает PoolState::Algebra и TickMap с сырыми тиками (как ждёт math/swap_algebra).
//! Читается по одному пулу (как Elastic/Bin): целевой Multicall3 идёт через call::view_many (node/call.rs), под общую
//! пачку load_batch.rs (ступени по словам компрессированного битмапа V3/CL) этот сырой битмап не ложится.
//! Кто вызывает: pools/load.rs (и через него load_batch.rs для Algebra).
//! От чего зависит: ctx.rs, node/call.rs (view/view_many, Multicall3 — решение 83), pools/abi_algebra.rs, load_cl.rs
//! (i24), ticks.rs, core/types (PoolState).
//! Настройки: dex.tick_words [bsc_dex] (сколько слов сырого битмапа читать в каждую сторону от текущего).
//!
//! Геттеры — @cryptoalgebra/v1.9-core (решение 23): globalState/liquidity/ticks/tickTable — IAlgebraPoolState.sol.
//! СЫРОЙ битмап (ключевое отличие от V3): TickTable индексирует сам тик — rowNumber = tick >> 8, bitNumber = tick & 0xFF
//! (TickTable.sol, БЕЗ деления на tickSpacing). Поэтому слово w покрывает тики [w·256, w·256+255], а тик бита b слова w
//! = w·256 + b (spacing не участвует). В карту берём тики с ненулевым liquidityTotal (как у V3/CL/Elastic). words
//! карты — прочитанные сырые слова: math/swap_algebra::window_ticks считает по ним границы (w·256 … w·256+255).
//! TickMap.spacing = 1: битмап Algebra индексирует каждый сырой тик (шаг бита = 1), covers()/modify() для Algebra не
//! вызываются (событие ликвидности → перечитывание целиком, events_algebra.rs) — поле хранит структуру карты, не
//! параметр пула.

use std::collections::BTreeMap;

use alloy_primitives::{Address, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_algebra::IAlgebraPool;
use super::load_cl::i24;
use super::ticks::{TickData, TickMap};

/// Состояние и карта тиков пула Algebra на блоке.
/// Принимает: адрес пула, блок-тег.
/// Ошибки: NotConfigured (dex.tick_words), узел, ответ не по ABI, тик вне int24.
pub async fn load(ctx: &Ctx, pool: Address, tag: &str) -> GatewayResult<(PoolState, TickMap)> {
    let gs = call::view(ctx, pool, &IAlgebraPool::globalStateCall {}, tag).await?;
    let liquidity = call::view(ctx, pool, &IAlgebraPool::liquidityCall {}, tag).await?;
    let tick = gs.tick.as_i32();
    let state = PoolState::Algebra {
        sqrt_price_x96: U256::from(gs.price),
        tick,
        liquidity,
        fee: u32::from(gs.fee),
    };
    // Окно СЫРЫХ слов битмапа вокруг текущего тика: слово тика = tick >> 8 (TickTable.sol), по n = dex.tick_words
    // в каждую сторону. Полоса цены dex.depth_band_pct для Algebra не разворачивается в число слов (слово сырого
    // битмапа = 256 тиков ≈ 2,56% цены — заметно шире компрессированного слова V3): если полоса выйдет за окно,
    // math/swap_algebra вернёт частичный итог (complete = false), и граф занизит глубину в безопасную сторону.
    let (lo, hi) = raw_word_range(ctx, tick)?;
    let calls: Vec<_> = (lo..=hi)
        .map(|w| IAlgebraPool::tickTableCall { wordPosition: w })
        .collect();
    let words = call::view_many(ctx, pool, &calls, tag).await?;
    let found = raw_ticks(lo, &words);
    // liquidityTotal и liquidityDelta каждого найденного сырого тика (как gross/net у V3/CL/Elastic).
    let tick_calls = found
        .iter()
        .map(|t| {
            Ok(IAlgebraPool::ticksCall {
                tick: i24(ctx, *t)?,
            })
        })
        .collect::<GatewayResult<Vec<_>>>()?;
    let infos = call::view_many(ctx, pool, &tick_calls, tag).await?;
    let mut map: BTreeMap<i32, TickData> = BTreeMap::new();
    for (t, i) in found.iter().zip(infos) {
        if i.liquidityTotal > 0 {
            map.insert(
                *t,
                TickData {
                    gross: i.liquidityTotal,
                    net: i.liquidityDelta,
                },
            );
        }
    }
    Ok((
        state,
        TickMap {
            spacing: 1,
            ticks: map,
            words: (lo, hi),
        },
    ))
}

/// Сырые слова битмапа вокруг текущего тика: [w − n, w + n] в пределах int16, где w = tick >> 8 (TickTable.sol:
/// rowNumber = shr(8, tick)), n = dex.tick_words. Для СЫРОГО тика деления на tickSpacing нет. Ошибки: NotConfigured
/// (dex.tick_words).
pub fn raw_word_range(ctx: &Ctx, tick: i32) -> GatewayResult<(i16, i16)> {
    let n = ctx.int("dex.tick_words")?.min(i16::MAX as u64) as i32;
    // tick >> 8 в Rust для i32 — арифметический сдвиг (floor(tick/256)); совпадает с shr(8, tick) EVM после усечения
    // в int16 для диапазона тиков Algebra (±887272): отрицательный тик даёт отрицательное слово.
    let w = tick >> 8;
    let lo = (w - n).max(i32::from(i16::MIN)) as i16;
    let hi = (w + n).min(i32::from(i16::MAX)) as i16;
    Ok((lo, hi))
}

/// Сырые тики по установленным битам слов битмапа: тик бита b слова w = w·256 + b (TickTable.sol, без tickSpacing).
/// Принимает: первое слово, слова. Ошибок нет.
pub fn raw_ticks(first: i16, words: &[U256]) -> Vec<i32> {
    let mut out = Vec::new();
    for (i, word) in words.iter().enumerate() {
        let w = i32::from(first) + i as i32;
        for bit in 0..256usize {
            if word.bit(bit) {
                out.push(w * 256 + bit as i32);
            }
        }
    }
    out
}
