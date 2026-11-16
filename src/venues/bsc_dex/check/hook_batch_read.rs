//! Файл: venues/bsc_dex/check/hook_batch_read.rs
//! Что делает: части пакетной пробы крючков (check/hook_batch.rs, решение «335 (В1, пакет)»), вынесены, чтобы файлы
//! были не больше ~200 строк: план чтения пула CL в пакете (cl_plan — slot0 и liquidity, слова битмапа вокруг тика в
//! памяти, данные установленных тиков по карте в памяти; те же вызовы, что ступени 1–3 pools/load_batch.rs), разбор
//! его ответов в запись на блоке (cl_parse — с проверкой, что битмап на блоке тот же и свежий тик внутри прочитанных
//! слов), выход квотера из ответа пакета (quoted).
//! Кто вызывает: check/hook_batch.rs.
//! От чего зависит: check/hook_batch_parse.rs, check/hooks_probe.rs (short), pools/{load, load_batch, load_batch_cl,
//! load_batch_ticks, load_batch_v2v3, load_v2v3, ticks}.rs, quote/quoter_calls.rs.
//! Настройки: через word_range — dex.tick_words, dex.depth_band_pct, dex.tick_read_band_pct [bsc_dex].

use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::families::Proto;
use super::super::pools::load::{self, Loaded};
use super::super::pools::load_batch::Want;
use super::super::pools::load_batch_v2v3::PoolStateTick;
use super::super::pools::load_v2v3::word_range;
use super::super::pools::registry::Entry;
use super::super::pools::ticks::TickMap;
use super::super::pools::{load_batch_cl, load_batch_ticks};
use super::super::quote::quoter_calls;
use super::hook_batch_parse::{self as parse, Fail};
use super::hooks_probe;

/// Как получить состояние пула на блоке пакета.
pub enum Read {
    /// Уже есть (верификация маршрута).
    Have(Box<Entry>),
    /// В пакете: индекс slot0, (индекс слов, слова), (индекс тиков, тики по памяти), tickSpacing.
    Cl(usize, (usize, (i16, i16)), (usize, Vec<i32>), i32),
    /// Обычное чтение на том же блоке.
    Apart,
}

/// Запрос чтения пула (как у verify.rs). Ошибок нет.
pub fn want(e: &Entry) -> Want<'_> {
    Want {
        family: e.family,
        reference: &e.pool.reference,
        spacing: e.pool.tick_spacing,
        step: e.pool.bin_step,
        usdt: e.usdt,
        token0: e.pool.token0,
        token1: e.pool.token1,
    }
}

/// План чтения пула CL в пакете (см. шапку) — вызовы дописываются в `calls`. Apart — вид не CL, нет карты тиков в
/// памяти или она не покрывает слова, не задана настройка. Ошибок нет.
pub fn cl_plan(ctx: &Ctx, e: &Entry, calls: &mut Vec<(Address, Bytes)>) -> Read {
    if !matches!(e.family.proto, Proto::InfinityCl | Proto::UniswapV4) {
        return Read::Apart;
    }
    let (Some(spacing), Some(m), PoolState::Cl { tick, .. }) =
        (e.pool.tick_spacing, e.ticks.as_ref(), &e.pool.state)
    else {
        return Read::Apart;
    };
    let Ok(range) = word_range(ctx, *tick, spacing) else {
        return Read::Apart;
    };
    let Some(pred) = parse::predicted(m, range) else {
        return Read::Apart;
    };
    let w = want(e);
    let (Ok(h), Ok(wc), Ok(tc)) = (
        load_batch_cl::head(ctx, &w),
        load_batch_cl::word_calls(ctx, &w, range),
        load_batch_ticks::tick_calls(ctx, &w, &pred),
    ) else {
        return Read::Apart;
    };
    let (hi, wi) = (calls.len(), calls.len() + h.len());
    let ti = wi + wc.len();
    calls.extend(h);
    calls.extend(wc);
    calls.extend(tc);
    Read::Cl(hi, (wi, range), (ti, pred), spacing)
}

/// Состояние пула CL на блоке из ответов пакета. None — ответа нет, битмап на блоке не тот, что в памяти, или
/// свежий тик ушёл за прочитанные слова (дочитать обычным чтением). Ошибок нет.
pub fn cl_parse(
    ctx: &Ctx,
    e: &Entry,
    read: &Read,
    ans: &[Result<Bytes, String>],
    block: u64,
) -> Option<Entry> {
    let Read::Cl(hi, (wi, range), (ti, pred), spacing) = read else {
        return None;
    };
    let got = |from: usize, n: usize| -> Option<Vec<Bytes>> {
        ans.get(from..from + n)?
            .iter()
            .map(|r| r.clone().ok())
            .collect()
    };
    let w = want(e);
    let PoolStateTick::Ticked(state, tick) = load_batch_cl::parse_head(&w, &got(*hi, 2)?).ok()?
    else {
        return None;
    };
    let n = usize::try_from(i32::from(range.1) - i32::from(range.0) + 1).ok()?;
    let set = load_batch_cl::set_ticks(&w, range.0, &got(*wi, n)?, *spacing).ok()?;
    let now = word_range(ctx, tick, *spacing).ok()?;
    if !parse::fits(now, *range) || !parse::same(pred, &set) {
        return None;
    }
    let ticks = load_batch_ticks::tick_data(&w, pred, &got(*ti, pred.len())?).ok()?;
    let l = Loaded {
        state,
        ticks: Some(TickMap {
            spacing: *spacing,
            ticks: ticks.into(),
            words: *range,
        }),
        bins: None,
        fee_pips: None,
    };
    Some(load::renewed(e, block, l))
}

/// Выход квотера из ответа пакета (см. шапку и hook_batch_parse classify). Ok(Err) — «не хватает ликвидности».
/// Ошибки: SwapReverted — откат квотера; BadResponse — ответ не по ABI, сбой узла на вызове, ответа нет.
pub fn quoted(
    ctx: &Ctx,
    e: &Entry,
    a: Option<&Result<Bytes, String>>,
) -> GatewayResult<Result<U256, String>> {
    let t = &e.family.title;
    match a {
        Some(Ok(b)) => quoter_calls::decode(e, b).map(Ok).ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                format!("{t}: ответ квотера не по ABI"),
            )
        }),
        Some(Err(text)) => match parse::classify(text) {
            Fail::Short(id) => Ok(Err(hooks_probe::short(id))),
            Fail::Revert => Err(ctx.err(ErrorCode::SwapReverted, format!("{t}: квотер: {text}"))),
            Fail::Fault => Err(ctx.err(ErrorCode::BadResponse, format!("{t}: квотер: {text}"))),
        },
        None => Err(ctx.err(
            ErrorCode::BadResponse,
            format!("{t}: нет ответа квотера в пакете"),
        )),
    }
}
