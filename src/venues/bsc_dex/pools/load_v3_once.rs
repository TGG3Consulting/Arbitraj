//! Файл: venues/bsc_dex/pools/load_v3_once.rs
//! Что делает: решение 315 — перечитывание пула V3 (ABI PancakeSwap V3 / Uniswap V3) ПО СОБЫТИЮ одним обращением к
//! узлу: голова (slot0, liquidity, [swapFee фабрики — решение 274], [fee() пула — решение 281]) и слова карты TickLens
//! одним eth_call к Multicall3.aggregate3 на блоке чтения. Последовательное чтение (load_v2v3.rs v3 + load.rs) делает
//! это не меньше чем тремя обращениями: слова зависят от текущего тика, а он — из ответа slot0. Здесь слова берутся
//! вокруг тика ЗАПИСИ, переданной вызывающим: в ветке off_map — запись после применения события (тик события,
//! events_v3.rs), в ветке Refresh (реорг, расхождение) — запись ДО события (событие не применено, listen_log.rs берёт
//! её до apply); итог от этого не зависит — по slot0 того же ответа всегда вычисляется настоящий диапазон (load_v2v3.rs word_range — та же функция, что у последовательного чтения): лежит
//! внутри прочитанного — берутся только его слова (ответ TickLens — по одному слову, лишние слова отбрасываются);
//! не лежит — второе обращение: слова настоящего диапазона тем же Multicall3 на том же блоке. Итог (состояние, тики,
//! диапазон слов, комиссия) — тот же, что у последовательного чтения на этом блоке: те же вызовы, тот же разбор
//! (load_batch_v2v3.rs head / parse_head / words / v3_ticks, fee_fresh.rs split — общий с пачечным чтением
//! load_batch.rs, без дублей). Ошибка узла (в том числе StaleData, решение 298) уходит как есть; откат вызова, который
//! входит в итог, — BadResponse (как у пачки); откат лишнего слова итог не трогает.
//! Включаемый: dex.event_refresh_one_call = да И задан dex.multicall3. Нет / не задана / нет Multicall3 / не V3 этих ABI
//! / у записи нет tickSpacing или тика V3 — путь не применяется (None), чтение прежнее.
//! Кто вызывает: pools/refresh_event.rs (read).
//! От чего зависит: node/multicall.rs (aggregate), node/call.rs (block_tag), load_batch_v2v3.rs, load_batch.rs (Want,
//! Miss), load_v2v3.rs (word_range), fee_fresh.rs (Extras, split), load.rs (Loaded), ticks.rs, registry.rs (Entry).
//! Настройки: dex.event_refresh_one_call [bsc_dex], dex.multicall3 [bsc_dex]; через вызываемые — dex.tick_lens,
//! dex.tick_words, dex.depth_band_pct, dex.tick_read_band_pct, dex.v3_fee_source, dex.factory.
//!
//! Официальный ABI (как в load_batch_v2v3.rs и load_v2v3.rs): slot0 / liquidity / TickLens.getPopulatedTicksInWord —
//! research/flow/bsc_dex.md, раздел 3 (IPancakeV3PoolState.sol#L21-L48, IUniswapV3PoolState.sol#L21-L32,
//! ITickLens.sol#L6-L24); Multicall3.aggregate3, Call3 и Result —
//! https://github.com/mds1/multicall3/blob/main/src/Multicall3.sol#L19-L35 (aggregate3 — #L98; node/multicall.rs).

use alloy_primitives::Bytes;

use crate::core::types::{PoolKind, PoolRef, PoolState};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call::block_tag;
use super::super::node::multicall;
use super::families::Proto;
use super::fee_fresh::{self, Extras};
use super::load::Loaded;
use super::load_batch::{Miss, Want, miss};
use super::load_batch_v2v3::{PoolStateTick, head, parse_head, v3_ticks, words};
use super::load_v2v3::word_range;
use super::registry::Entry;
use super::ticks::TickMap;

/// Итог разбора одного ответа: всё прочитано, или голова есть, а слова нужны другие (второе обращение).
pub enum Got {
    Done(Loaded),
    Words {
        state: PoolState,
        fee: Option<u32>,
        range: (i16, i16),
    },
}

/// Читает пул записи одним обращением (см. шапку). Принимает: окружение, запись, блок чтения.
/// ABI: slot0, liquidity — IPancakeV3PoolState.sol#L21-L48 / IUniswapV3PoolState.sol#L21-L32; слова —
/// ITickLens.sol#L6-L24 (research/flow/bsc_dex.md, раздел 3); одно обращение — Multicall3.aggregate3
/// (https://github.com/mds1/multicall3/blob/main/src/Multicall3.sol#L98).
/// Возвращает: Some — прочитано; None — путь не применяется (чтение прежнее). Ошибки: узел (как есть), BadResponse
/// (откат нужного вызова, ответ не по ABI), NotConfigured (dex.tick_lens, dex.tick_words, dex.factory).
pub async fn read(ctx: &Ctx, e: &Entry, at: u64) -> GatewayResult<Option<Loaded>> {
    if !ctx.flag("dex.event_refresh_one_call").unwrap_or(false) {
        return Ok(None);
    }
    let Some(mc) = multicall::address(ctx) else {
        return Ok(None);
    };
    let p = &e.pool;
    let applies = e.family.kind == PoolKind::V3
        && matches!(e.family.proto, Proto::PancakeV3 | Proto::UniswapV3)
        && matches!(p.reference, PoolRef::Contract(_))
        // Решение 483: Ramses CL — не TickLens (load_ramses.rs).
        && !super::ramses::is_cl(e.family);
    let (Some(spacing), PoolState::V3 { tick, .. }, true) = (p.tick_spacing, &p.state, applies)
    else {
        return Ok(None);
    };
    let w = Want {
        family: e.family,
        reference: &p.reference,
        spacing: Some(spacing),
        step: p.bin_step,
        usdt: e.usdt,
        token0: p.token0,
        token1: p.token1,
    };
    let tag = block_tag(at);
    let x = Extras::of(ctx, e.family);
    let mut calls = head(ctx, &w, x)?;
    let head_len = calls.len();
    let pred = word_range(ctx, *tick, spacing)?;
    calls.extend(words(ctx, &w, pred)?);
    let ans = multicall::aggregate(ctx, mc, &calls, &tag).await?;
    match assemble(&w, x, head_len, pred, &ans, &|t| {
        word_range(ctx, t, spacing)
    })? {
        Got::Done(l) => Ok(Some(l)),
        Got::Words { state, fee, range } => {
            let more = multicall::aggregate(ctx, mc, &words(ctx, &w, range)?, &tag).await?;
            let ticks = v3_ticks(&used(&more)?)?;
            Ok(Some(loaded(state, spacing, ticks, range, fee)))
        }
    }
}

/// Разбор ответа (чистая логика) по официальному ABI: slot0 / liquidity — IPancakeV3PoolState.sol#L21-L48,
/// IUniswapV3PoolState.sol#L21-L32 (load_batch_v2v3.rs parse_head); getPopulatedTicksInWord — ITickLens.sol#L6-L24
/// (v3_ticks); Result aggregate3 — Multicall3.sol#L19-L35. Голова — первые `head_len` ответов (fee() 281 отделяется fee_fresh::split по тому же
/// признаку `x`, что строил вызовы), дальше — слова `pred`. Настоящий диапазон — `actual(тик slot0)`; внутри `pred` —
/// Done с тиками только его слов, иначе — Words (нужно второе обращение).
/// Ошибки: откат или нехватка ответа головы или нужного слова, ответ не по ABI, нет tickSpacing, ошибка `actual`.
pub fn assemble(
    w: &Want,
    x: Extras,
    head_len: usize,
    pred: (i16, i16),
    ans: &[Result<Bytes, String>],
    actual: &dyn Fn(i32) -> Result<(i16, i16), Miss>,
) -> Result<Got, Miss> {
    let n = head_len.min(ans.len());
    let h = used(&ans[..n])?;
    if h.len() < head_len {
        return Err(miss(format!(
            "{}: ответов головы {} из {head_len}",
            w.family.title,
            h.len()
        )));
    }
    let (rest, fee) = fee_fresh::split(x.fresh, &h)?;
    let PoolStateTick::Ticked(state, tick) = parse_head(w, rest)? else {
        return Err(miss(format!("{}: не V3", w.family.title)));
    };
    let range = actual(tick)?;
    if range.0 < pred.0 || range.1 > pred.1 {
        return Ok(Got::Words { state, fee, range });
    }
    let from = head_len + (i32::from(range.0) - i32::from(pred.0)) as usize;
    let to = from + (i32::from(range.1) - i32::from(range.0)) as usize + 1;
    let Some(slice) = ans.get(from..to) else {
        return Err(miss(format!(
            "{}: ответов слов меньше, чем вызовов",
            w.family.title
        )));
    };
    let ticks = v3_ticks(&used(slice)?)?;
    let Some(spacing) = w.spacing else {
        return Err(miss(format!("{}: у пула нет tickSpacing", w.family.title)));
    };
    Ok(Got::Done(loaded(state, spacing, ticks, range, fee)))
}

/// Байты ответов; первый откат — причина. Ошибки: откат вызова.
fn used(ans: &[Result<Bytes, String>]) -> Result<Vec<Bytes>, Miss> {
    ans.iter().map(|r| r.clone().map_err(miss)).collect()
}

/// Прочитанное состояние, как у load.rs (ветка V3). Ошибок нет.
fn loaded(
    state: PoolState,
    spacing: i32,
    ticks: std::collections::BTreeMap<i32, super::ticks::TickData>,
    words: (i16, i16),
    fee_pips: Option<u32>,
) -> Loaded {
    Loaded {
        state,
        ticks: Some(TickMap {
            spacing,
            ticks: ticks.into(),
            words,
        }),
        bins: None,
        fee_pips,
    }
}
