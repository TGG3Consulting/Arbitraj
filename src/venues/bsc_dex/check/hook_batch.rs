//! Файл: venues/bsc_dex/check/hook_batch.rs
//! Что делает: решение «335 (В1, пакет)» — проба крючков B/C всех пулов доли перед сделкой (exec/trade_probe.rs
//! hooks_again) на ОДНОМ блоке и одним кругом до узла, без лишнего чтения:
//! - графовый маршрут: предторговая верификация (решение 83, quote/verify.rs run) уже перечитала пулы маршрута на
//!   блоке Bv — её свежие записи (RouteFresh, Quoting.route_fresh) берутся как есть, в пакете только вызовы квотера
//!   на Bv (по два на пул: покупка, продажа);
//! - иначе (прямой, умный маршрут, пара через промежуточную; верификации нет): блок H — голова node/heads.rs; в одном
//!   пакете на H для каждого пула Infinity CL / Uniswap V4 — slot0 и liquidity, слова битмапа вокруг тика в памяти
//!   (те же слова, что у обычного чтения, pools/load_v2v3.rs word_range), данные установленных тиков этих слов по
//!   карте в памяти и оба вызова квотера. На H проверяется (check/hook_batch_parse.rs), что битмап этих слов тот же и
//!   слова вокруг свежего тика внутри прочитанных — тогда состояние на H полное (как pools/load_batch.rs: ступени
//!   1–3 те же вызовы и тот же разбор); иначе пул дочитывается обычным чтением на том же H (лишние круги — только
//!   при смене тиков). Infinity Bin и пул без карты в памяти — обычное чтение на H одновременно с пакетом.
//!
//! Пакет — node/call.rs try_many: при dex.multicall3 — один eth_call Multicall3.aggregate3 (allowFailure; геттеры
//! менеджера / StateView — view, вызывающий не важен; квотер ловит свой откат QuoteSwap внутри try/catch и
//! возвращается обычно — infinity-periphery 56a7a4f5 src/pool-cl/lens/CLQuoter.sol#L27-L38, aggregate3 зовёт через
//! call — Multicall3.sol#L98), иначе одной пачкой JSON-RPC. Оба — один круг до узла.
//! Виртуальный квотер (решение 186) — прежний путь по одному (hooks_probe).
//! Кто вызывает: exec/trade_probe.rs (hooks_again). Чтение CL в пакете и разбор ответов — check/hook_batch_read.rs.
//! От чего зависит: check/hooks_probe.rs, check/hook_batch_read.rs, pools/{load, load_batch}.rs,
//! quote/{quoter_calls, virtual_quoter}.rs, node/call.rs, pools/fusion_book.rs (isolated — решение 463).
//! Настройки: dex.multicall3 [bsc_dex] (через try_many); check.probe_usdt, dex.quoter, dex.tick_words — через вызываемые.

use std::collections::HashMap;
use std::sync::Arc;

use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::pools::load_batch::{self, Want};
use super::super::pools::registry::{Entry, Registry};
use super::super::pools::{fusion_book, load};
use super::super::tokens::Tokens;
use super::hook_batch_read::{Read, cl_parse, cl_plan, quoted, want};
use super::hooks_probe::{self, Measured, Plan};

/// Свежие записи пулов маршрута, перечитанные предторговой верификацией на блоке `block` (решение 83).
#[derive(Debug, Clone)]
pub struct RouteFresh {
    pub block: u64,
    pub pools: Arc<HashMap<PoolRef, Entry>>,
}

/// Что делает: пакетная проба (см. шапку). Принимает: окружение, токены, реестр, пулы с крючком B/C (записи из
/// памяти) и план пробы каждого (точка на сумме сделки — решение «335 (лесенка влияния)»), свежие записи маршрута
/// (если были), голову. Возвращает: блок пробы и итог по каждому пулу (Measured, как hooks_probe). Ошибки: узел на
/// пакете целиком (по пулу — в его итоге).
pub async fn measure(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    pools: &[(Entry, Plan)],
    fresh: Option<&RouteFresh>,
    head: u64,
) -> GatewayResult<(u64, Vec<Measured>)> {
    let fresh = fresh.filter(|f| {
        pools
            .iter()
            .all(|(e, _)| f.pools.contains_key(&e.pool.reference))
    });
    let block = fresh.map_or(head, |f| f.block);
    let tag = call::block_tag(block);
    let mut out: Vec<Option<Measured>> = pools.iter().map(|_| None).collect();
    // (пул, как читать, индекс первого вызова квотера плана)
    let mut plans: Vec<(usize, Read, usize)> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for (i, (mem, plan)) in pools.iter().enumerate() {
        let base = fresh.map_or(mem, |f| &f.pools[&mem.pool.reference]);
        let asks = if matches!(mem.family.variant,super::super::pools::family_variant::Variant::SupernovaIntegral|super::super::pools::family_variant::Variant::CypherIntegral)
        {
            Vec::new()
        } else {
            match hooks_probe::requests(ctx, mem, plan) {
                Ok(a) => a,
                Err(err) => {
                    out[i] = Some(Err(err));
                    continue;
                }
            }
        };
        let read = if hooks_probe::individual(ctx, mem) {
            // Виртуальный квотер (решение 186) — не eth_call квотера: состояние в пакете не читается, проба по одному.
            match fresh {
                Some(_) => Read::Have(Box::new(base.clone())),
                None => Read::Apart,
            }
        } else {
            match fresh {
                Some(_) => Read::Have(Box::new(base.clone())),
                None => cl_plan(ctx, mem, &mut calls),
            }
        };
        plans.push((i, read, calls.len()));
        if !hooks_probe::individual(ctx, mem) {
            calls.extend(asks);
        }
    }
    let apart: Vec<usize> = (0..plans.len())
        .filter(|k| matches!(plans[*k].1, Read::Apart))
        .collect();
    let wants: Vec<Want> = apart.iter().map(|k| want(&pools[plans[*k].0].0)).collect();
    let (ans, sep) = tokio::join!(
        call::try_many(ctx, &calls, &tag),
        // Решение 463 (повторное ревью t236): оракул THENA Fusion этого чтения — не в книгу (fusion_book.rs).
        fusion_book::isolated(load_batch::states(ctx, tokens, reg, &wants, block))
    );
    let ans = ans?;
    let mut sep: HashMap<usize, _> = apart.into_iter().zip(sep).collect();
    // Состояния на блоке; пулы, у которых предсказание тиков не сошлось, — дочитать обычным чтением (одно на всех).
    let mut states: Vec<Option<Entry>> = Vec::with_capacity(plans.len());
    for (k, (i, read, _)) in plans.iter().enumerate() {
        let e = &pools[*i].0;
        states.push(match read {
            Read::Have(f) => Some((**f).clone()),
            Read::Apart => match sep.remove(&k) {
                Some(Ok(l)) => Some(load::renewed(e, block, l)),
                Some(Err(err)) => {
                    out[*i] = Some(Err(err));
                    None
                }
                None => None,
            },
            Read::Cl(..) => cl_parse(ctx, e, read, &ans, block),
        });
    }
    let again: Vec<usize> = (0..plans.len())
        .filter(|k| states[*k].is_none() && out[plans[*k].0].is_none())
        .collect();
    let wants: Vec<Want> = again.iter().map(|k| want(&pools[plans[*k].0].0)).collect();
    for (k, r) in again
        .iter()
        .zip(fusion_book::isolated(load_batch::states(ctx, tokens, reg, &wants, block)).await)
    {
        let i = plans[*k].0;
        match r {
            Ok(l) => states[*k] = Some(load::renewed(&pools[i].0, block, l)),
            Err(err) => out[i] = Some(Err(err)),
        }
    }
    for (k, (i, _, q)) in plans.iter().enumerate() {
        let (Some(st), None) = (&states[k], &out[*i]) else {
            continue;
        };
        let (e, plan) = &pools[*i];
        let r = if hooks_probe::individual(ctx, e) {
            hooks_probe::measure_plan(ctx, st, plan, &tag).await
        } else {
            let n = plan.buy.len() + plan.sell.len();
            let remote = (0..n).map(|j| quoted(ctx, e, ans.get(q + j))).collect();
            hooks_probe::judge_plan(ctx, st, plan, remote)
        };
        out[*i] = Some(r);
    }
    let lost = || Err(ctx.err(ErrorCode::BadResponse, "пул не измерен в пакете пробы"));
    Ok((
        block,
        out.into_iter().map(|o| o.unwrap_or_else(lost)).collect(),
    ))
}
