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
//! Решение 295: пары многомонетного пула (PoolRef::Multi) одного контракта — одно чтение на все (load_site.rs).
//! Кто вызывает: pools/reload.rs (load_many); one_by_one — ещё reload_many.rs (решение 298); решение 331 —
//! load_batch_par.rs (states на подмножестве без видов «по одному», one_by_one, batch, miss).
//! От чего зависит: load_batch_v2v3.rs, load_batch_cl.rs, load_batch_ticks.rs, load.rs (Loaded, state),
//! load_v2v3.rs (word_range), ticks.rs, fee_fresh.rs (решение 281: свежая fee() V3 — в Loaded.fee_pips).
//! Настройки: dex.load_batch, dex.multicall3 [bsc_dex] (без него — пачка JSON-RPC); через вызываемые — dex.tick_words,
//! dex.depth_band_pct, dex.tick_read_band_pct (word_range, решение 296), dex.tick_lens, dex.state_view, dex.bin_depth_usd.
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
use super::fee_fresh::{self, Extras};
use super::load::{self, Loaded};
use super::load_site;
use super::load_batch_cl as cl;
use super::load_batch_ticks as ticks3;
use super::load_batch_v2v3::{PoolStateTick, head, parse_head, run, v3_ticks, words};
use super::load_v2v3::word_range;
use super::registry::Registry;
use super::ticks::TickMap;

/// Что читать: вид, ссылка, tickSpacing, binStep, USDT и token1 (Bin), как у load::state. Copy — решение 331
/// (load_batch_par.rs: подмножество пулов для ступеней Multicall3).
#[derive(Clone, Copy)]
pub struct Want<'a> {
    pub family: &'static Family,
    pub reference: &'a PoolRef,
    pub spacing: Option<i32>,
    pub step: Option<u32>,
    pub usdt: Address,
    /// Пара пула (token0 < token1): token0 нужен StableSwap — индексы пары среди монет пула (решение 122 F).
    pub token0: Address,
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

/// Состояния пулов на блоке. Принимает: окружение, токены, реестр (решение 306: курс встречного к USDT для лесенки —
/// load.rs state), пулы, блок. Возвращает: по каждому — Loaded или причина. Ошибок нет.
pub async fn states(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    wants: &[Want<'_>],
    block: u64,
) -> Vec<Result<Loaded, Miss>> {
    let tag = block_tag(block);
    let size = match batch(ctx) {
        Ok(n) => n,
        Err(e) => return wants.iter().map(|_| Err(e.clone())).collect(),
    };
    let mut out: Out = wants.iter().map(|_| None).collect();
    // Решение 295: пары многомонетного пула (PoolRef::Multi) — одно чтение на контракт для всех его пар в пачке
    // (load_site.rs); остальные ссылки сюда не попадают (группы пусты — чтение прежнее).
    for (contract, idx) in load_site::groups(wants) {
        let w0 = &wants[idx[0]];
        let pairs: Vec<(Address, Address)> = idx.iter().map(|&i| (wants[i].token0, wants[i].token1)).collect();
        let got = load_site::states(ctx, tokens, w0.family, contract, &pairs, block).await;
        for (i, r) in idx.into_iter().zip(got) {
            out[i] = Some(r);
        }
    }
    // Bin, KyberSwap Elastic, DODO PMM, Algebra, Solidly и Slipstream — по одному (см. шапку; у Elastic обход списка
    // тиков не ложится на ступени по словам, у DODO чтение кривой — свой набор вызовов, у Algebra (THENA Fusion) сжатый битмап
    // tickTable и tickSpacing пула читаются своим загрузчиком (load_algebra.rs, решение 463), не ступенями V3/CL; у Solidly свой Pair-интерфейс (metadata + getFee фабрики);
    // у Slipstream — форк Uni V3, но slot0 без feeProtocol и тики прямыми геттерами tickBitmap/ticks, а не через
    // TickLens, как ступени V3; под общие ступени V2/V3 не ложится; load_slipstream.rs).
    for (i, w) in wants.iter().enumerate() {
        if out[i].is_some() {
            continue;
        }
        if one_by_one(w.family) {
            let r = load::state(
                ctx,
                tokens,
                reg,
                w.family,
                w.reference,
                w.spacing,
                w.step,
                w.usdt,
                (w.token0, w.token1),
                block,
            )
            .await;
            out[i] = Some(r);
        }
    }
    // Ступень 1: головы. Добавки V3 (swapFee 274, fee() 281) — один раз на пул: тот же признак и в разбор (ревью 281).
    let extras: Vec<Extras> = wants.iter().map(|w| Extras::of(ctx, w.family)).collect();
    let first: Stage = wants
        .iter()
        .enumerate()
        .map(|(i, w)| match out[i] {
            Some(_) => Vec::new(),
            None => head(ctx, w, extras[i]).unwrap_or_else(|why| {
                out[i] = Some(Err(why));
                Vec::new()
            }),
        })
        .collect();
    let heads = run(ctx, &first, &tag, size).await;
    // Разобранные головы V3/CL: состояние, тик, tickSpacing.
    let mut parsed: Vec<Option<(PoolState, i32, i32)>> = wants.iter().map(|_| None).collect();
    // Свежая fee() пула (решение 281, fee_fresh.rs): только у DEX с dex.v3_fee_source = pool_fee_each_read.
    let mut fees: Vec<Option<u32>> = wants.iter().map(|_| None).collect();
    for (i, w) in wants.iter().enumerate() {
        if out[i].is_some() {
            continue;
        }
        let head = heads[i]
            .as_ref()
            .map_err(|t| miss(t.clone()))
            .and_then(|b| fee_fresh::split(extras[i].fresh, b))
            .and_then(|(b, fee)| {
                fees[i] = fee;
                parse_head(w, b)
            });
        match head {
            Ok(PoolStateTick::V2(state)) => {
                out[i] = Some(Ok(Loaded {
                    state,
                    ticks: None,
                    bins: None,
                    fee_pips: None,
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
                    ticks: ticks.into(),
                    words,
                }),
                bins: None,
                fee_pips: fees[i],
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

/// Что делает: вид читается по одному последовательными eth_call (load::state), а не ступенями Multicall3 — список и
/// причины см. в states. Ещё — reload_many.rs (решение 298: такому пулу — свой свежий блок).
/// Принимает: вид пула. Возвращает: true — по одному. Ошибок нет.
pub fn one_by_one(f: &Family) -> bool {
    f.kind == PoolKind::Bin
        || f.proto == Proto::KyberElastic
        || f.proto.is_dodo()
        || f.proto == Proto::Algebra
        || f.proto == Proto::AlgebraIntegral
        || f.proto == Proto::AlgebraDirFee
        || f.proto.is_solidly()
        || f.proto == Proto::Slipstream
        // Решение 483: Ramses CL (v3_fork_ramses) — свои ABI и тики без TickLens (load_ramses.rs).
        || super::ramses::is_cl(f)
        || f.proto.is_stable()
        || f.proto == Proto::WooPP
        || f.proto == Proto::Maverick
        || f.proto == Proto::MetricOmm
        || matches!(f.proto, Proto::IziSwap | Proto::BiswapV3)
        || f.proto == Proto::KyberDmm
        || f.proto == Proto::Smardex
        || f.proto == Proto::BalancerV2
        || f.proto == Proto::BalancerV3
        || f.proto == Proto::LbV21
        || f.proto == Proto::FluidT1
        || f.proto.is_curve_crypto()
        || f.proto == Proto::CamelotV2
        || f.proto == Proto::AeroBasic
        || f.proto == Proto::LbV20
        || f.proto == Proto::LbV22
        || f.proto == Proto::Ekubo
        || f.proto == Proto::Wombat
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
