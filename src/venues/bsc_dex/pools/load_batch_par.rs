//! Файл: venues/bsc_dex/pools/load_batch_par.rs
//! Что делает: решение 331 — чтение состояния многих пулов на ОДНОМ блоке, где пулы видов «по одному»
//! (load_batch::one_by_one: Bin, LB, Algebra, Algebra Integral, Slipstream, Solidly, DODO, KyberSwap Elastic,
//! KyberSwap DMM, SmarDex, Balancer, стейблсвапы, WooFi, Maverick, Metric, iZiSwap / Biswap V3) и группы пар
//! многомонетных пулов (PoolRef::Multi, load_site.rs) читаются ОДНОВРЕМЕННО — не больше `par` чтений сразу — и
//! одновременно со ступенями Multicall3 остальных пулов (V2, V3, CL, V4 — load_batch::states). Раньше
//! (load_batch::states) — друг за другом в цикле, и ступени Multicall3 ждали их всех.
//! Итог тот же: те же функции чтения (load::state, load_site::states, load_batch::states) на том же блоке, тот же
//! порядок ответов; не задана dex.load_batch — у всех пулов та же причина, что раньше. Внутренности чтения вида не
//! меняются (load_*.rs). Каждый запрос к узлу проходит очередь темпа узла (node/pace.rs: bsc.node_http_rps, классы
//! gateway/urgency.rs) с классом задачи вызывающего: чтения идут в той же задаче tokio (поток future, не spawn).
//! Кто вызывает: quote/verify.rs и quote/cycle_legs.rs (verify.read_parallel_max).
//! От чего зависит: load_batch.rs (Want, states, one_by_one, batch, miss), load.rs (state), load_site.rs (groups,
//! states), ctx.rs, tokens.rs, registry.rs.
//! Настройки: значение предела читает вызывающий (limit); через вызываемые — dex.load_batch и настройки чтения видов.

use std::collections::HashSet;

use alloy_primitives::Address;
use futures_util::StreamExt;

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::load::{self, Loaded};
use super::load_batch::{self, Miss, Want};
use super::load_site;
use super::registry::Registry;

/// Что делает: предел одновременных чтений из целочисленной настройки площадки `name`. Принимает: окружение, имя.
/// Возвращает: Some(n) — задана и больше нуля; None — не задана (или 0, или не читается) — прежнее чтение по очереди.
/// Ошибок нет.
pub fn limit(ctx: &Ctx, name: &str) -> Option<usize> {
    match ctx.int(name) {
        Ok(n) if n > 0 => Some(usize::try_from(n).unwrap_or(usize::MAX)),
        _ => None,
    }
}

/// Что делает: состояния пулов на блоке — при `par` = None ровно load_batch::states (как раньше), при Some(n) —
/// states (ниже, одновременно). Принимает: окружение, токены, реестр, пулы, блок, предел. Возвращает: по каждому пулу
/// (в том же порядке) Loaded или причина. Ошибок нет.
pub async fn states_with(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    wants: &[Want<'_>],
    block: u64,
    par: Option<usize>,
) -> Vec<Result<Loaded, Miss>> {
    match par {
        Some(n) => states(ctx, tokens, reg, wants, block, n).await,
        None => load_batch::states(ctx, tokens, reg, wants, block).await,
    }
}

/// Одна работа одновременного чтения: группа пар многомонетного пула (номер группы) или один пул «по одному».
enum Job {
    Site(usize),
    Solo(usize),
}

/// Что делает: состояния пулов на блоке одновременно (см. шапку). Принимает: окружение, токены, реестр, пулы, блок,
/// предел одновременных чтений (0 — как 1). Возвращает: по каждому пулу (в том же порядке) Loaded или причина.
/// Ошибок нет.
pub async fn states(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    wants: &[Want<'_>],
    block: u64,
    par: usize,
) -> Vec<Result<Loaded, Miss>> {
    // Как load_batch::states: без dex.load_batch — у всех пулов одна причина (и узел не спрашивается).
    if let Err(e) = load_batch::batch(ctx) {
        return wants.iter().map(|_| Err(e.clone())).collect();
    }
    let sites = load_site::groups(wants);
    let in_site: HashSet<usize> = sites
        .iter()
        .flat_map(|(_, idx)| idx.iter().copied())
        .collect();
    let (solo, rest): (Vec<usize>, Vec<usize>) = (0..wants.len())
        .filter(|i| !in_site.contains(i))
        .partition(|&i| load_batch::one_by_one(wants[i].family));
    let rest_wants: Vec<Want<'_>> = rest.iter().map(|&i| wants[i]).collect();
    let jobs: Vec<Job> = (0..sites.len())
        .map(Job::Site)
        .chain(solo.iter().copied().map(Job::Solo))
        .collect();
    let sites_ref = &sites;
    let reads = futures_util::stream::iter(jobs)
        .map(move |j| async move {
            match j {
                Job::Site(g) => {
                    let (contract, idx) = &sites_ref[g];
                    site(ctx, tokens, wants, *contract, idx, block).await
                }
                Job::Solo(i) => {
                    let w = &wants[i];
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
                    vec![(i, r)]
                }
            }
        })
        .buffered(par.max(1))
        .collect::<Vec<_>>();
    // Ступени Multicall3 остальных пулов — одновременно с чтениями «по одному» (раньше ждали их всех).
    let batched = async {
        if rest_wants.is_empty() {
            Vec::new()
        } else {
            load_batch::states(ctx, tokens, reg, &rest_wants, block).await
        }
    };
    let (reads, batched) = tokio::join!(reads, batched);
    let mut out: Vec<Option<Result<Loaded, Miss>>> = wants.iter().map(|_| None).collect();
    for (i, r) in reads.into_iter().flatten() {
        out[i] = Some(r);
    }
    for (&i, r) in rest.iter().zip(batched) {
        out[i] = Some(r);
    }
    out.into_iter()
        .map(|o| {
            o.unwrap_or_else(|| {
                Err(load_batch::miss(
                    "состояние пула не собрано (внутренняя ошибка)",
                ))
            })
        })
        .collect()
}

/// Что делает: одна группа пар многомонетного пула — одно чтение на контракт (load_site::states, как в
/// load_batch::states). Принимает: окружение, токены, пулы, контракт, индексы группы, блок. Возвращает: (индекс, итог)
/// по каждой паре группы. Ошибок нет.
async fn site(
    ctx: &Ctx,
    tokens: &Tokens,
    wants: &[Want<'_>],
    contract: Address,
    idx: &[usize],
    block: u64,
) -> Vec<(usize, Result<Loaded, Miss>)> {
    let Some(&first) = idx.first() else {
        return Vec::new();
    };
    let pairs: Vec<(Address, Address)> = idx
        .iter()
        .map(|&i| (wants[i].token0, wants[i].token1))
        .collect();
    let got = load_site::states(ctx, tokens, wants[first].family, contract, &pairs, block).await;
    idx.iter().copied().zip(got).collect()
}
