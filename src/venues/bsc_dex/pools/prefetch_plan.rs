//! Файл: venues/bsc_dex/pools/prefetch_plan.rs
//! Что делает: решение 325 — какие view-вызовы пула прочитать ЗАРАНЕЕ при перечитывании по событию
//! (pools/prefetch_read.rs): first — известное до первого обращения (голова, слова вокруг тика записи), more —
//! зависящее от прочитанных ответов (слова настоящего диапазона, тики по битам слов, тики списка). Вызовы — теми же
//! функциями, что у пачечного чтения (load_batch_v2v3.rs head / words, load_batch_cl.rs, load_batch_ticks.rs), или теми
//! же sol!, что у чтения по одному (prefetch_ticks.rs, prefetch_integral.rs) — те же байты, код чтения берёт ответ из
//! книги. Только подсказка: не угадали — вызов к узлу, итог прежний. Ошибка построения — пусто.
//! Виды: V2, V3 (PancakeSwap V3 / Uniswap V3 ABI), CL (Infinity CL, Uniswap V4), Slipstream, Algebra V1.9, Algebra
//! Integral, KyberSwap Elastic, DODO PMM — здесь; остальные — только вызовы прошлого чтения (prefetch_memo.rs).
//! Ревью 325 (Н5): план — Ask: нужное коду чтения (useful) и вспомогательное (extra — группы, которых код чтения сам
//! не делает: дерево тиков Integral). prefetch_read.rs pick берёт вспомогательное, только пока частей обращения не
//! больше, чем нужных вызовов. Elastic — только голова и getTicksInRange (их код чтения делает по одному); ticks() списка
//! код чтения читает ОДНИМ view_many (load_elastic.rs) — заранее не читаются; memo у Elastic и Integral не берётся.
//! Так обращений не больше, чем до решения 325, при любом dex.load_batch (доказательство — шапка prefetch_read.rs).
//! Кто вызывает: pools/prefetch_read.rs.
//! От чего зависит: load_batch.rs (Want), load_batch_v2v3.rs, load_batch_cl.rs, load_batch_ticks.rs, load_v2v3.rs
//! (word_range), fee_fresh.rs (Extras, split), prefetch_ticks.rs, prefetch_integral.rs, abi_elastic.rs, abi_dodo.rs,
//! load_dodo.rs (FEE_TRADER), prefetch_ask.rs (Ask), node/prefetch.rs (Book), node/call.rs (decode).
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
use super::load_v2v3::word_range;
use super::prefetch_ask::Ask;
use super::registry::Entry;
use super::{prefetch_algebra_dir, prefetch_integral, prefetch_ticks};

/// Вызов: контракт и ABI-данные. Ошибок нет.
pub fn enc<C: SolCall>(to: Address, c: &C) -> Call {
    (to, Bytes::from(c.abi_encode()))
}

/// Разобранный ответ вызова из книги. Нет ответа или не по ABI — None. Ошибок нет.
pub fn got<C: SolCall>(b: &Book, to: Address, c: &C) -> Option<C::Return> {
    call::decode::<C>(&b.get(to, &Bytes::from(c.abi_encode()))?).ok()
}

/// Брать ли вызовы прошлого чтения (prefetch_memo.rs): у Elastic и Algebra Integral — нет (ревью 325, Н5: у Elastic в
/// них ticks() всех тиков, которые код чтения берёт одним view_many; у Integral план точнее прошлого раза). Ошибок нет.
pub fn memo(e: &Entry) -> bool {
    !matches!(e.family.proto, Proto::KyberElastic | Proto::AlgebraIntegral)
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

/// Вызовы, известные до первого обращения. Принимает: окружение, запись (тик записи: у off_map и «только комиссии» —
/// после события, у Refresh — до события; это только подсказка, ревью 325 Н6). Ошибок нет.
pub fn first(ctx: &Ctx, e: &Entry) -> Ask {
    if let (Proto::AlgebraIntegral, PoolRef::Contract(a)) = (e.family.proto, &e.pool.reference) {
        return prefetch_integral::first(e, *a);
    }
    first_calls(ctx, e).into()
}

/// first у видов, где всё заранее читаемое — нужное коду чтения. Ошибок нет.
fn first_calls(ctx: &Ctx, e: &Entry) -> Vec<Call> {
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
        (Proto::AlgebraDirFee, PoolRef::Contract(a)) => prefetch_algebra_dir::first(ctx, e, *a),
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
pub fn more(ctx: &Ctx, e: &Entry, b: &Book) -> Ask {
    if staged(e) {
        return staged_more(ctx, e, b).unwrap_or_default().into();
    }
    match (e.family.proto, &e.pool.reference) {
        (Proto::Slipstream, PoolRef::Contract(a)) => {
            prefetch_ticks::slip_more(ctx, e, *a, b).into()
        }
        (Proto::Algebra, PoolRef::Contract(a)) => prefetch_ticks::algebra_more(ctx, *a, b).into(),
        (Proto::AlgebraDirFee, PoolRef::Contract(a)) => {
            prefetch_algebra_dir::more(ctx, *a, b).into()
        }
        (Proto::AlgebraIntegral, PoolRef::Contract(a)) => prefetch_integral::more(ctx, *a, b),
        // KyberSwap Elastic: ticks() списка — одним view_many кода чтения (ревью 325, Н5), заранее не читаются.
        _ => Ask::default(),
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

/// Elastic: вызов всех тиков списка у читателя (как load_elastic.rs). Пусто — нет dex.tick_reader. Ошибок нет.
fn elastic_all(ctx: &Ctx, e: &Entry, pool: Address) -> Vec<Call> {
    let (Ok(reader), Ok(start)) = (
        ctx.dex_addr("dex.tick_reader", e.family.id),
        i24(ctx, MIN_TICK),
    ) else {
        return Vec::new();
    };
    let c = ITicksFeesReader::getTicksInRangeCall {
        pool,
        startTick: start,
        length: 0,
    };
    vec![enc(reader, &c)]
}
