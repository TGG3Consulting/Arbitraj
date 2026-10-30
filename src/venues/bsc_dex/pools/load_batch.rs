//! Файл: venues/bsc_dex/pools/load_batch.rs
//! Что делает: чтение состояния МНОГИХ пулов на одном блоке пачками (решение 61): вместо 3–4 eth_call на каждый пул
//! по очереди — ступени, на каждой вызовы всех пулов вместе через Multicall3.aggregate3 (node/call.rs try_chunks,
//! частями по dex.load_batch): 1) V2 getReserves, V3 slot0 + liquidity, CL slot0 + liquidity; 2) слова тиков (V3 —
//! TickLens по словам, CL — слова битмапа); 3) данные установленных тиков CL. Те же функции и тот же разбор по ABI,
//! что у чтения по одному (load_v2v3.rs, load_cl.rs) — правила те же: любой вызов пула откатился, не по ABI или часть
//! пачки не дошла (узел) — пул не прочитан с причиной (повтор — reload.rs). Bin читается по одному (load.rs):
//! поиск непустых бинов последовательный — следующий вызов зависит от ответа предыдущего. KyberSwap Elastic
//! (решение 81) тоже по одному (load_elastic.rs): обход связного списка тиков — не битмап, под ступени по словам
//! не ложится.
//! Кто вызывает: pools/reload.rs (load_many).
//! От чего зависит: load_batch_v2v3.rs, load_batch_cl.rs, load_batch_ticks.rs, load.rs (Loaded, state),
//! load_v2v3.rs (word_range), ticks.rs.
//! Настройки: dex.load_batch, dex.multicall3 [bsc_dex] (без него — пачка JSON-RPC); через вызываемые — dex.tick_words,
//! dex.tick_lens, dex.state_view, dex.bin_depth_usd.
//!
//! Функции контрактов — те же, что в load_v2v3.rs и load_cl.rs (их шапки — ссылки на официальные интерфейсы);
//! Multicall3.aggregate3 — node/multicall.rs.

use alloy_primitives::{Address, Bytes};

use crate::core::types::{PoolKind, PoolRef, PoolState};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call::block_tag;
use super::super::tokens::Tokens;
use super::families::{Family, Proto};
use super::load::{self, Loaded};
use super::load_batch_cl as cl;
use super::load_batch_ticks as ticks3;
use super::load_batch_v2v3::{PoolStateTick, head, parse_head, run, v3_ticks, words};
use super::load_v2v3::word_range;
use super::ticks::TickMap;

/// Что читать: вид, ссылка, tickSpacing, binStep, USDT и token1 (Bin), как у load::state.
pub struct Want<'a> {
    pub family: &'static Family,
    pub reference: &'a PoolRef,
    pub spacing: Option<i32>,
    pub step: Option<u32>,
    pub usdt: Address,
    pub token1: Address,
}

/// Вызовы одной ступени по пулам: (контракт, данные).
pub type Stage = Vec<Vec<(Address, Bytes)>>;

/// Причина, по которой пул не прочитан: ошибка шлюза (у «не задана настройка» — имя в поле setting).
pub type Miss = crate::gateway::errors::GatewayError;
/// Причина из текста (ответ не по форме, откат, сбой части пачки) — BadResponse без имени настройки. Ошибок нет.
pub fn miss(text: impl Into<String>) -> Miss {
    super::super::errors::err(crate::gateway::errors::ErrorCode::BadResponse, text)
}

/// Ответы ступени по пулам: байты каждого вызова пула или причина (первый откат / сбой части).
pub type Answers = Vec<Result<Vec<Bytes>, String>>;

/// Итог по каждому пулу, пока собирается (None — ещё в работе).
pub type Out = Vec<Option<Result<Loaded, Miss>>>;

/// Состояния пулов на блоке. Принимает: пулы, блок. Возвращает: по каждому — Loaded или причина. Ошибок нет.
pub async fn states(
    ctx: &Ctx,
    tokens: &Tokens,
    wants: &[Want<'_>],
    block: u64,
) -> Vec<Result<Loaded, Miss>> {
    let tag = block_tag(block);
    let size = match batch(ctx) {
        Ok(n) => n,
        Err(e) => return wants.iter().map(|_| Err(e.clone())).collect(),
    };
    let mut out: Out = wants.iter().map(|_| None).collect();
    // Bin, KyberSwap Elastic, DODO PMM и Algebra — по одному (см. шапку; у Elastic обход списка тиков не ложится на
    // ступени по словам, у DODO чтение кривой — свой набор вызовов, у Algebra сырой битмап tickTable индексирует сам
    // тик — не компрессированные слова V3/CL, под общие ступени не ложится; load_algebra.rs).
    for (i, w) in wants.iter().enumerate() {
        if w.family.kind == PoolKind::Bin
            || w.family.proto == Proto::KyberElastic
            || w.family.proto.is_dodo()
            || w.family.proto == Proto::Algebra
        {
            let r = load::state(
                ctx,
                tokens,
                w.family,
                w.reference,
                w.spacing,
                w.step,
                w.usdt,
                w.token1,
                block,
            )
            .await;
            out[i] = Some(r);
        }
    }
    // Ступень 1: головы.
    let first: Stage = wants
        .iter()
        .enumerate()
        .map(|(i, w)| match out[i] {
            Some(_) => Vec::new(),
            None => head(ctx, w).unwrap_or_else(|why| {
                out[i] = Some(Err(why));
                Vec::new()
            }),
        })
        .collect();
    let heads = run(ctx, &first, &tag, size).await;
    // Разобранные головы V3/CL: состояние, тик, tickSpacing.
    let mut parsed: Vec<Option<(PoolState, i32, i32)>> = wants.iter().map(|_| None).collect();
    for (i, w) in wants.iter().enumerate() {
        if out[i].is_some() {
            continue;
        }
        let head = heads[i]
            .as_ref()
            .map_err(|t| miss(t.clone()))
            .and_then(|b| parse_head(w, b));
        match head {
            Ok(PoolStateTick::V2(state)) => {
                out[i] = Some(Ok(Loaded {
                    state,
                    ticks: None,
                    bins: None,
                }))
            }
            Ok(PoolStateTick::Ticked(s, tick)) => match w.spacing {
                Some(sp) => parsed[i] = Some((s, tick, sp)),
                None => {
                    out[i] = Some(Err(miss(format!(
                        "{}: у пула нет tickSpacing",
                        w.family.title
                    ))))
                }
            },
            Err(why) => out[i] = Some(Err(why)),
        }
    }
    // Ступень 2: слова (V3 — тики TickLens, CL — битмап).
    let mut ranges: Vec<(i16, i16)> = vec![(0, 0); wants.len()];
    let second: Stage = wants
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let Some((_, tick, spacing)) = parsed[i] else {
                return Vec::new();
            };
            let calls = word_range(ctx, tick, spacing).and_then(|r| {
                ranges[i] = r;
                words(ctx, w, r)
            });
            calls.unwrap_or_else(|why| {
                parsed[i] = None;
                out[i] = Some(Err(why));
                Vec::new()
            })
        })
        .collect();
    let words_ans = run(ctx, &second, &tag, size).await;
    let mut cl_wait: ticks3::Waiting = wants.iter().map(|_| None).collect();
    for (i, w) in wants.iter().enumerate() {
        let Some((state, _, spacing)) = parsed[i].take() else {
            continue;
        };
        let ans = match &words_ans[i] {
            Ok(b) => b,
            Err(why) => {
                out[i] = Some(Err(miss(why.clone())));
                continue;
            }
        };
        if w.family.kind == PoolKind::V3 {
            let words = ranges[i];
            out[i] = Some(v3_ticks(ans).map(|ticks| Loaded {
                state,
                ticks: Some(TickMap {
                    spacing,
                    ticks,
                    words,
                }),
                bins: None,
            }));
        } else {
            match cl::set_ticks(w, ranges[i].0, ans, spacing) {
                Ok(t) => cl_wait[i] = Some((state, t, spacing, ranges[i])),
                Err(why) => out[i] = Some(Err(why)),
            }
        }
    }
    // Ступень 3: данные тиков CL.
    ticks3::finish(ctx, wants, cl_wait, &mut out, &tag, size).await;
    out.into_iter()
        .map(|o| o.unwrap_or_else(|| Err(miss("состояние пула не собрано (внутренняя ошибка)"))))
        .collect()
}

/// Размер части пачки: dex.load_batch (вызовов в одном eth_call Multicall3). Ошибки: NotConfigured (не задана или 0).
pub fn batch(ctx: &Ctx) -> GatewayResult<usize> {
    match ctx.int("dex.load_batch")? {
        0 => Err(ctx.not_set(
            "dex.load_batch",
            "dex.load_batch = 0 — задайте число больше нуля",
        )),
        n => Ok(n as usize),
    }
}
