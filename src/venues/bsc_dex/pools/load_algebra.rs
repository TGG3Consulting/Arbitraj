//! Файл: venues/bsc_dex/pools/load_algebra.rs
//! Что делает: чтение состояния пула THENA Fusion (вид thena_fusion — код Algebra V1 с шагом тиков TICK_SPACING,
//! решение 463) на заданном блоке: globalState (цена, тик, текущая динамическая комиссия), liquidity (одна
//! ликвидность — reinvestL у Algebra нет), tickSpacing() и карта тиков по СЖАТОМУ битмапу tickTable(int16) в словах
//! вокруг текущего тика + liquidityTotal / liquidityDelta каждого тика методом пула ticks(). Собирает PoolState::Algebra
//! и TickMap (spacing = tickSpacing пула, words — сжатые слова), как ждёт math/swap_algebra (режим NextTick::Row).
//! Читается по одному пулу (как Elastic/Bin): целевой Multicall3 идёт через call::view_many (node/call.rs).
//! Кто вызывает: pools/load.rs (и через него load_batch.rs для Algebra); row_range / row_ticks — ещё
//! pools/prefetch_ticks.rs (заранее читаемые вызовы, решение 325) и pools/off_map_algebra.rs (решение 320);
//! tick_map — ещё pools/load_algebra_dir.rs (решение 444: Algebra V1.9 Camelot V3 — СЫРОЙ битмап, шаг карты 1).
//! Решение 463 (T463-02): комиссия состояния — та, что получит своп СЛЕДУЮЩЕГО блока (pools/fusion_fee.rs next_fee:
//! оракул пула, math/fee_algebra_v1.rs), не globalState.fee.
//! От чего зависит: ctx.rs, node/call.rs (view/view_many, Multicall3 — решение 83), pools/abi_algebra.rs, fusion_fee.rs, load_cl.rs
//! (i24), load_v2v3.rs (band_words, read_band_words), ticks.rs, core/types (PoolState).
//! Настройки: dex.tick_words [bsc_dex] (сколько слов битмапа читать в каждую сторону от текущего);
//! dex.tick_read_band_pct [bsc_dex] (необязательная, решение 296: не меньше слов полосы чтения при шаге пула);
//! dex.tick_read_band_algebra_pct [bsc_dex] (необязательная, решение 319: задана — полоса чтения Algebra берётся из неё
//! вместо dex.tick_read_band_pct).
//!
//! ФАКТ кода (решение 463): исходник пула THENA Fusion на Sourcify (chain 56, пул 0x1b9a…ef7d и деплоер 0xc89F…7358 —
//! полное совпадение байткода с исходником THENA) — линия AlgebraV1: TickTable, TICK_SPACING и цикл обмена совпадают
//! с cryptoalgebra/AlgebraV1 master, но дословной копией master он не является (комиссия сообщества у THENA — uint16,
//! предел 1000; в master — uint8, 250): Constants.TICK_SPACING = 60;
//! TickTable.toggleTick: `tick /= Constants.TICK_SPACING` (сжатие), rowNumber = сжатый тик >> 8, bitNumber = & 0xFF;
//! nextTickInTheSameRow сжимает тик (округление к −∞) и возвращает uncompressAndBoundTick (× TICK_SPACING, в пределах
//! MIN_TICK / MAX_TICK). Algebra V1.9 (Camelot V3, решения 444, 461) — битмап СЫРОЙ (TickTable rowNumber = tick >> 8
//! без сжатия): те же функции при шаге 1 (row_range / row_ticks при spacing = 1 — сырые слова и тики).
//! Запись своего узла 20.11 (tests/data/bsc_thena_fusion_2026-11-20): у 7 из 8 пулов сырые слова
//! вокруг tick >> 8 пустые, сжатые — 4…99 тиков, у всех liquidityTotal > 0. Поэтому бит b слова w — тик
//! (w·256 + b)·spacing, слово тика — floor(tick / spacing) >> 8 (TickMap::word_of). Шаг — tickSpacing() пула (у
//! записанных пулов = 60 = Constants.TICK_SPACING), не константа бота. Геттеры — IAlgebraPoolState.sol (abi_algebra.rs).

use std::collections::BTreeMap;

use alloy_primitives::{Address, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_algebra::IAlgebraPool;
use super::fusion_fee::{self, Head};
use super::load_cl::i24;
use super::load_v2v3::{band_words, read_band_words};
use super::ticks::{TickData, TickMap};

/// Состояние и карта тиков пула THENA Fusion на блоке.
/// Принимает: адрес пула, известный tickSpacing пула (ревью t236: он неизменен — Constants.TICK_SPACING кода пула;
/// после первого чтения хранится в Pool.tick_spacing записи, load.rs built / renewed; None — первое чтение: читается
/// tickSpacing()), блок-тег.
/// Ошибки: NotConfigured (dex.tick_words), узел, ответ не по ABI, BadResponse (tickSpacing ≤ 0, тик вне int24).
pub async fn load(
    ctx: &Ctx,
    pool: Address,
    known: Option<i32>,
    tag: &str,
) -> GatewayResult<(PoolState, TickMap)> {
    let gs = call::view(ctx, pool, &IAlgebraPool::globalStateCall {}, tag).await?;
    let liquidity = call::view(ctx, pool, &IAlgebraPool::liquidityCall {}, tag).await?;
    let spacing = match known {
        Some(s) => s,
        None => call::view(ctx, pool, &IAlgebraPool::tickSpacingCall {}, tag)
            .await?
            .as_i32(),
    };
    let spacing = spacing_of(ctx, spacing)?;
    let tick = gs.tick.as_i32();
    // Решение 463 (T463-02): комиссия свопа СЛЕДУЮЩЕГО блока (первый своп блока пересчитывает её по оракулу), а не
    // globalState.fee — с ней считают котировка и глубина (pools/fusion_fee.rs).
    let head = Head {
        tick,
        index: gs.timepointIndex,
        fee: u32::from(gs.fee),
    };
    let fee = fusion_fee::next_fee(ctx, pool, head, tag).await?;
    let state = PoolState::Algebra {
        sqrt_price_x96: U256::from(gs.price),
        tick,
        liquidity,
        fee,
        fee_otz: None,
    };
    Ok((state, tick_map(ctx, pool, tick, spacing, tag).await?))
}

/// Карта тиков пула Algebra вокруг тика `tick` на блоке. Общая для THENA Fusion (сжатый битмап, `spacing` =
/// tickSpacing пула) и Algebra V1.9 с направленными комиссиями (решение 444, load_algebra_dir.rs: сырой битмап —
/// `spacing` = 1; TickTable и ticks() — cryptoalgebra/AlgebraV1.9 a45417ed, IAlgebraPoolState.sol:72, :87). Принимает:
/// окружение, пул, текущий тик, шаг карты (> 0), блок-тег. Возвращает: TickMap. Ошибки: NotConfigured
/// (dex.tick_words), узел, ответ не по ABI, тик вне int24.
pub async fn tick_map(
    ctx: &Ctx,
    pool: Address,
    tick: i32,
    spacing: i32,
    tag: &str,
) -> GatewayResult<TickMap> {
    // Окно СЖАТЫХ слов битмапа вокруг текущего тика (при spacing = 1 — сырых): слово = floor(tick / spacing) >> 8, по n
    // слов в каждую сторону (row_range). Полоса цены dex.depth_band_pct сюда не разворачивается (как до решения 463):
    // если обмен выйдет за окно, math/swap_algebra вернёт частичный итог (complete = false). Решение 320: тик, ушедший
    // после Swap за окно, — pools/off_map_algebra.rs.
    let (lo, hi) = row_range(ctx, tick, spacing)?;
    let calls: Vec<_> = (lo..=hi)
        .map(|w| IAlgebraPool::tickTableCall { wordPosition: w })
        .collect();
    let words = call::view_many(ctx, pool, &calls, tag).await?;
    let found = row_ticks(lo, &words, spacing);
    // liquidityTotal и liquidityDelta каждого найденного тика (как gross/net у V3/CL/Elastic).
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
    Ok(TickMap {
        spacing,
        ticks: map.into(),
        words: (lo, hi),
    })
}

/// Проверенный шаг тиков пула. Принимает: окружение, ответ tickSpacing(). Возвращает: шаг > 0.
/// Ошибки: BadResponse (шаг ≤ 0 — битмап по нему не разложить).
pub fn spacing_of(ctx: &Ctx, spacing: i32) -> GatewayResult<i32> {
    if spacing <= 0 {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            format!("Algebra tickSpacing {spacing} ≤ 0"),
        ));
    }
    Ok(spacing)
}

/// Сжатые слова битмапа вокруг текущего тика: [w − n, w + n] в пределах int16, где w = floor(tick / spacing) >> 8
/// (TickTable.sol AlgebraV1: сжатие, затем rowNumber = shr(8, tick)), n = большее из dex.tick_words и слов полосы
/// чтения при шаге пула (algebra_band_words). Принимает: окружение, тик, шаг (> 0). Ошибки: NotConfigured
/// (dex.tick_words).
pub fn row_range(ctx: &Ctx, tick: i32, spacing: i32) -> GatewayResult<(i16, i16)> {
    let n = ctx.int("dex.tick_words")?.min(i16::MAX as u64) as i32;
    let n = n.max(algebra_band_words(ctx, spacing));
    let w = i32::from(TickMap::word_of(tick, spacing.max(1)));
    let lo = (w - n).max(i32::from(i16::MIN)) as i16;
    let hi = (w + n).min(i32::from(i16::MAX)) as i16;
    Ok((lo, hi))
}

/// Решение 319 (решение 463 — при шаге пула): слова битмапа Algebra в каждую сторону по полосе чтения. Задана
/// dex.tick_read_band_algebra_pct — слова по ней (band_words), и dex.tick_read_band_pct на Algebra тогда не действует;
/// не задана или не читается — read_band_words по dex.tick_read_band_pct. Принимает: окружение, шаг пула. Возвращает:
/// число слов (0 — полосы нет). Ошибок нет.
pub fn algebra_band_words(ctx: &Ctx, spacing: i32) -> i32 {
    ctx.dec("dex.tick_read_band_algebra_pct").map_or_else(
        |_| read_band_words(ctx, spacing),
        |pct| band_words(pct, spacing),
    )
}

/// Тики по установленным битам сжатых слов: тик бита b слова w = (w·256 + b)·spacing (TickTable.sol AlgebraV1,
/// uncompress = × TICK_SPACING). Принимает: первое слово, слова, шаг. Ошибок нет.
pub fn row_ticks(first: i16, words: &[U256], spacing: i32) -> Vec<i32> {
    let mut out = Vec::new();
    for (i, word) in words.iter().enumerate() {
        let w = i32::from(first) + i as i32;
        for bit in 0..256usize {
            if word.bit(bit) {
                out.push((w * 256 + bit as i32) * spacing);
            }
        }
    }
    out
}
