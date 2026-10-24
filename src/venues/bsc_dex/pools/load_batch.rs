//! Файл: venues/bsc_dex/pools/load_batch.rs
//! Что делает: чтение состояния МНОГИХ пулов на одном блоке пачками (решение 61): вместо 3–4 eth_call на каждый пул
//! по очереди — ступени, на каждой вызовы всех пулов вместе через Multicall3.aggregate3 (node/call.rs try_chunks,
//! частями по dex.verify_batch): 1) V2 getReserves, V3 slot0 + liquidity, CL slot0 + liquidity; 2) слова тиков (V3 —
//! TickLens по словам, CL — слова битмапа); 3) данные установленных тиков CL. Те же функции и тот же разбор по ABI,
//! что у чтения по одному (load_v2v3.rs, load_cl.rs) — правила те же: любой вызов пула откатился, не по ABI или часть
//! пачки не дошла (узел) — пул не прочитан с причиной (повтор — reload.rs). Bin читается по одному (load.rs):
//! поиск непустых бинов последовательный — следующий вызов зависит от ответа предыдущего.
//! Кто вызывает: pools/reload.rs (load_many).
//! От чего зависит: load_batch_v2v3.rs, load_batch_cl.rs, load.rs (Loaded, state),
//! load_v2v3.rs (word_range), verify.rs (batch), ticks.rs.
//! Настройки: dex.verify_batch, dex.multicall3 [bsc_dex] (без него — пачка JSON-RPC); через вызываемые — dex.tick_words,
//! dex.tick_lens, dex.state_view, dex.bin_depth_usd.
//!
//! Функции контрактов — те же, что в load_v2v3.rs и load_cl.rs (их шапки — ссылки на официальные интерфейсы);
//! Multicall3.aggregate3 — node/multicall.rs.

use alloy_primitives::{Address, Bytes};

use crate::core::types::{PoolKind, PoolRef, PoolState};

use super::super::ctx::Ctx;
use super::super::node::call::block_tag;
use super::super::tokens::Tokens;
use super::families::Family;
use super::load::{self, Loaded};
use super::load_batch_cl as cl;
use super::load_batch_v2v3::{PoolStateTick, head, parse_head, run, v3_ticks, words};
use super::load_v2v3::word_range;
use super::ticks::TickMap;
use super::verify;

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

/// Ответы ступени по пулам: байты каждого вызова пула или причина (первый откат / сбой части).
pub type Answers = Vec<Result<Vec<Bytes>, String>>;

/// Состояния пулов на блоке. Принимает: пулы, блок. Возвращает: по каждому — Loaded или причина. Ошибок нет.
pub async fn states(
    ctx: &Ctx,
    tokens: &Tokens,
    wants: &[Want<'_>],
    block: u64,
) -> Vec<Result<Loaded, String>> {
    let tag = block_tag(block);
    let size = match verify::batch(ctx) {
        Ok(n) => n,
        Err(e) => return wants.iter().map(|_| Err(e.message.clone())).collect(),
    };
    let mut out: Vec<Option<Result<Loaded, String>>> = wants.iter().map(|_| None).collect();
    // Bin — по одному (см. шапку).
    for (i, w) in wants.iter().enumerate() {
        if w.family.kind == PoolKind::Bin {
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
            out[i] = Some(r.map_err(|e| e.message));
        }
    }
    // Ступень 1: головы.
    let first: Stage = wants
        .iter()
        .enumerate()
        .map(|(i, w)| {
            if out[i].is_some() {
                Vec::new()
            } else {
                head(ctx, w).unwrap_or_else(|why| {
                    out[i] = Some(Err(why));
                    Vec::new()
                })
            }
        })
        .collect();
    let heads = run(ctx, &first, &tag, size).await;
    let mut parsed: Vec<Option<(PoolState, i32)>> = wants.iter().map(|_| None).collect();
    for (i, w) in wants.iter().enumerate() {
        if out[i].is_some() {
            continue;
        }
        match heads[i]
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|b| parse_head(w, b))
        {
            Ok(PoolStateTick::V2(s)) => {
                out[i] = Some(Ok(Loaded {
                    state: s,
                    ticks: None,
                    bins: None,
                }))
            }
            Ok(PoolStateTick::Ticked(s, tick)) => parsed[i] = Some((s, tick)),
            Err(why) => out[i] = Some(Err(why)),
        }
    }
    // Ступень 2: слова (V3 — тики TickLens, CL — битмап).
    let mut ranges: Vec<(i16, i16)> = vec![(0, 0); wants.len()];
    let second: Stage = wants
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let Some((_, tick)) = parsed[i] else {
                return Vec::new();
            };
            let spacing = w.spacing.unwrap_or_default();
            let calls = word_range(ctx, tick, spacing)
                .map_err(|e| e.message)
                .and_then(|r| {
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
    let mut cl_wait: Vec<Option<(PoolState, Vec<i32>)>> = wants.iter().map(|_| None).collect();
    for (i, w) in wants.iter().enumerate() {
        let Some((state, _)) = parsed[i].take() else {
            continue;
        };
        let spacing = w.spacing.unwrap_or_default();
        let ans = match &words_ans[i] {
            Ok(b) => b,
            Err(why) => {
                out[i] = Some(Err(why.clone()));
                continue;
            }
        };
        if w.family.kind == PoolKind::V3 {
            out[i] = Some(v3_ticks(ans).map(|ticks| Loaded {
                state,
                ticks: Some(TickMap {
                    spacing,
                    ticks,
                    words: ranges[i],
                }),
                bins: None,
            }));
        } else {
            match cl::set_ticks(w, ranges[i].0, ans) {
                Ok(t) => cl_wait[i] = Some((state, t)),
                Err(why) => out[i] = Some(Err(why)),
            }
        }
    }
    // Ступень 3: данные тиков CL.
    let third: Stage = wants
        .iter()
        .enumerate()
        .map(|(i, w)| match &cl_wait[i] {
            Some((_, t)) => cl::tick_calls(ctx, w, t).unwrap_or_else(|why| {
                cl_wait[i] = None;
                out[i] = Some(Err(why));
                Vec::new()
            }),
            None => Vec::new(),
        })
        .collect();
    let infos = run(ctx, &third, &tag, size).await;
    for (i, w) in wants.iter().enumerate() {
        let Some((state, t)) = cl_wait[i].take() else {
            continue;
        };
        let ticks = infos[i]
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|b| cl::tick_data(w, &t, b));
        out[i] = Some(ticks.map(|ticks| Loaded {
            state,
            ticks: Some(TickMap {
                spacing: w.spacing.unwrap_or_default(),
                ticks,
                words: ranges[i],
            }),
            bins: None,
        }));
    }
    out.into_iter()
        .map(|o| o.unwrap_or_else(|| Err("состояние пула не собрано (внутренняя ошибка)".into())))
        .collect()
}
