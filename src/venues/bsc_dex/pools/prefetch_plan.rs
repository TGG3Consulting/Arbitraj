//! Файл: venues/bsc_dex/pools/prefetch_plan.rs
//! Что делает: решение 325 — какие view-вызовы пула прочитать ЗАРАНЕЕ при перечитывании по событию
//! (pools/prefetch_read.rs): first — что известно до первого обращения (голова пула и слова карты вокруг тика записи),
//! more — что зависит от уже прочитанных ответов (слова настоящего диапазона по тику ответа, данные тиков по битам
//! слов, тики списка). Вызовы строятся теми же функциями, что у пачечного чтения (load_batch_v2v3.rs head / words,
//! load_batch_cl.rs head / word_calls / set_ticks, load_batch_ticks.rs tick_calls), или теми же вызовами sol!, что у
//! чтения по одному (prefetch_ticks.rs, prefetch_integral.rs) — тогда данные вызова те же байты, и код чтения берёт
//! ответ из книги. Это только подсказка: не угадали — вызов идёт к узлу, итог прежний. Ошибка построения — пустой
//! список (подсказки нет).
//! Виды: V2, V3 (PancakeSwap V3 / Uniswap V3 ABI), CL (Infinity CL, Uniswap V4), Slipstream, Algebra V1.9, Algebra
//! Integral, KyberSwap Elastic, DODO PMM — здесь; остальные — только вызовы прошлого чтения (prefetch_memo.rs).
//! Кто вызывает: pools/prefetch_read.rs.
//! От чего зависит: load_batch.rs (Want), load_batch_v2v3.rs, load_batch_cl.rs, load_batch_ticks.rs, load_v2v3.rs
//! (word_range), fee_fresh.rs (Extras, split), prefetch_ticks.rs, prefetch_integral.rs, abi_elastic.rs, abi_dodo.rs,
//! load_dodo.rs (FEE_TRADER), load_elastic.rs (parse_ticks), node/prefetch.rs (Book), node/call.rs (decode).
//! Настройки: через вызываемые — dex.tick_words, dex.tick_read_band_pct, dex.depth_band_pct, dex.tick_lens,
//! dex.state_view, dex.v3_fee_source, dex.tick_reader, dex.integral_tick_steps.
//!
//! ABI — те же, что у чтения по одному: V3 — research/flow/bsc_dex.md, раздел 3; CL — research/flow/bsc_v4.md, раздел 4;
//! Elastic — IPoolStorage.sol#L28-L101 и TicksFeesReader.getTicksInRange (load_elastic.rs); DODO — load_dodo.rs (DODO V2
//! IDODOV2 getPMMStateForCall, getUserFeeRate, _BASE_TOKEN_, _QUOTE_TOKEN_).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::{PoolKind, PoolRef, PoolState};

use super::super::ctx::Ctx;
use super::super::math::tick_math::MIN_TICK;
use super::super::node::call;
use super::super::node::prefetch::{Book, Call};
use super::abi_dodo::IDodoPool;
use super::abi_elastic::{IElasticPool, ITicksFeesReader};
use super::families::Proto;
use super::fee_fresh::{self, Extras};
use super::load_batch::Want;
use super::load_batch_cl as cl;
use super::load_batch_ticks::tick_calls;
use super::load_batch_v2v3::{PoolStateTick, head, parse_head, words};
use super::load_cl::i24;
use super::load_dodo::FEE_TRADER;
use super::load_elastic::parse_ticks;
use super::load_v2v3::word_range;
use super::registry::Entry;
use super::{prefetch_integral, prefetch_ticks};

/// Вызов: контракт и ABI-данные. Ошибок нет.
pub fn enc<C: SolCall>(to: Address, c: &C) -> Call {
    (to, Bytes::from(c.abi_encode()))
}

/// Разобранный ответ вызова из книги. Нет ответа или не по ABI — None. Ошибок нет.
pub fn got<C: SolCall>(b: &Book, to: Address, c: &C) -> Option<C::Return> {
    call::decode::<C>(&b.get(to, &Bytes::from(c.abi_encode()))?).ok()
}

/// Ответы списка вызовов из книги; хотя бы одного нет — None. Ошибок нет.
pub fn all(b: &Book, calls: &[Call]) -> Option<Vec<Bytes>> {
    calls.iter().map(|(to, d)| b.get(*to, d)).collect()
}

/// Что читать пулу записи, как load.rs (Want пачки). Ошибок нет.
fn want(e: &Entry) -> Want<'_> {
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

/// Ступенчатое чтение V2/V3/CL, как у пачки (load_batch.rs). Ошибок нет.
fn staged(e: &Entry) -> bool {
    match e.family.kind {
        PoolKind::V2 | PoolKind::Cl => !super::load_batch::one_by_one(e.family),
        PoolKind::V3 => matches!(e.family.proto, Proto::PancakeV3 | Proto::UniswapV3),
        _ => false,
    }
}

/// Вызовы, известные до первого обращения. Принимает: окружение, запись (тик записи — после события). Ошибок нет.
pub fn first(ctx: &Ctx, e: &Entry) -> Vec<Call> {
    let w = want(e);
    let p = &e.pool;
    if staged(e) {
        let Ok(mut l) = head(ctx, &w, Extras::of(ctx, e.family)) else {
            return Vec::new();
        };
        let tick = match p.state {
            PoolState::V3 { tick, .. } | PoolState::Cl { tick, .. } => tick,
            _ => return l,
        };
        if let Some(sp) = p.tick_spacing
            && let Ok(calls) = word_range(ctx, tick, sp).and_then(|r| words(ctx, &w, r))
        {
            l.extend(calls);
        }
        return l;
    }
    match (e.family.proto, &p.reference) {
        (Proto::Slipstream, PoolRef::Contract(a)) => prefetch_ticks::slip_first(ctx, e, *a),
        (Proto::Algebra, PoolRef::Contract(a)) => prefetch_ticks::algebra_first(ctx, e, *a),
        (Proto::AlgebraIntegral, PoolRef::Contract(a)) => prefetch_integral::first(ctx, e, *a),
        (Proto::KyberElastic, PoolRef::Contract(a)) => {
            let mut l = vec![
                enc(*a, &IElasticPool::getPoolStateCall {}),
                enc(*a, &IElasticPool::getLiquidityStateCall {}),
                enc(*a, &IElasticPool::swapFeeUnitsCall {}),
            ];
            l.extend(elastic_all(ctx, e, *a));
            l
        }
        (pr, PoolRef::Contract(a)) if pr.is_dodo() => vec![
            enc(*a, &IDodoPool::getPMMStateForCallCall {}),
            enc(*a, &IDodoPool::getUserFeeRateCall { user: FEE_TRADER }),
            enc(*a, &IDodoPool::_BASE_TOKEN_Call {}),
            enc(*a, &IDodoPool::_QUOTE_TOKEN_Call {}),
        ],
        _ => Vec::new(),
    }
}

/// Вызовы, зависящие от уже прочитанных ответов (книга). Пусто — добирать нечего. Ошибок нет.
pub fn more(ctx: &Ctx, e: &Entry, b: &Book) -> Vec<Call> {
    if staged(e) {
        return staged_more(ctx, e, b).unwrap_or_default();
    }
    match (e.family.proto, &e.pool.reference) {
        (Proto::Slipstream, PoolRef::Contract(a)) => prefetch_ticks::slip_more(ctx, e, *a, b),
        (Proto::Algebra, PoolRef::Contract(a)) => prefetch_ticks::algebra_more(ctx, *a, b),
        (Proto::AlgebraIntegral, PoolRef::Contract(a)) => prefetch_integral::more(ctx, *a, b),
        (Proto::KyberElastic, PoolRef::Contract(a)) => {
            let Some((to, c)) = elastic_all_call(ctx, e, *a) else {
                return Vec::new();
            };
            let Some(list) = got(b, to, &c) else {
                return Vec::new();
            };
            parse_ticks(&list)
                .iter()
                .filter_map(|t| i24(ctx, *t).ok())
                .map(|tick| enc(*a, &IElasticPool::ticksCall { tick }))
                .collect()
        }
        _ => Vec::new(),
    }
}

/// V2/V3/CL: по голове из книги — слова настоящего диапазона; по словам CL — данные установленных тиков.
/// Возвращает: None — ответов головы или слов ещё нет. Ошибок нет.
fn staged_more(ctx: &Ctx, e: &Entry, b: &Book) -> Option<Vec<Call>> {
    let w = want(e);
    let x = Extras::of(ctx, e.family);
    let h = all(b, &head(ctx, &w, x).ok()?)?;
    let (rest, _) = fee_fresh::split(x.fresh, &h).ok()?;
    let PoolStateTick::Ticked(_, tick) = parse_head(&w, rest).ok()? else {
        return None;
    };
    let sp = e.pool.tick_spacing?;
    let range = word_range(ctx, tick, sp).ok()?;
    let mut calls = words(ctx, &w, range).ok()?;
    if e.family.kind == PoolKind::Cl
        && let Some(ans) = all(b, &calls)
    {
        let set = cl::set_ticks(&w, range.0, &ans, sp).ok()?;
        calls.extend(tick_calls(ctx, &w, &set).ok()?);
    }
    Some(calls)
}

/// Elastic: вызов всех тиков списка у читателя (как load_elastic.rs). None — нет dex.tick_reader. Ошибок нет.
fn elastic_all_call(
    ctx: &Ctx,
    e: &Entry,
    pool: Address,
) -> Option<(Address, ITicksFeesReader::getTicksInRangeCall)> {
    let reader = ctx.dex_addr("dex.tick_reader", e.family.id).ok()?;
    let c = ITicksFeesReader::getTicksInRangeCall {
        pool,
        startTick: i24(ctx, MIN_TICK).ok()?,
        length: 0,
    };
    Some((reader, c))
}

/// Elastic: тот же вызов списком (для first). Ошибок нет.
fn elastic_all(ctx: &Ctx, e: &Entry, pool: Address) -> Vec<Call> {
    elastic_all_call(ctx, e, pool)
        .map(|(to, c)| vec![enc(to, &c)])
        .unwrap_or_default()
}
