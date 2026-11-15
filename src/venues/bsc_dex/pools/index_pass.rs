//! Файл: venues/bsc_dex/pools/index_pass.rs
//! Что делает: решение 295 — проход проверки справочника (index::take): режим dex.multi_pair (multi_pair::retune),
//! проход строк справочника с ключом строки multi_pair::dir_key (у DEX вне режима — строка пула, как до решения),
//! развёртка недостающих пар взятых многомонетных пулов (multi_pair::expand) и одна проверка новых строк в том же
//! вызове. Проверка кандидатов — параметром (на узле — verify::verify; тест — разборы на записанных ответах).
//! Кто вызывает: pools/index.rs (take), tests/bsc_dex_multi_pair.rs (take_with).
//! От чего зависит: index.rs (Took, record, summary), verify.rs, families.rs, multi_pair.rs, order.rs, directory.
//! Настройки: через вызываемые — dex.verify_batch, dex.multi_pair, dex.* [DEX].

use std::collections::{BTreeSet, HashSet};
use std::future::Future;
use std::pin::Pin;

use alloy_primitives::Address;

use crate::core::types::DirectoryStatus;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::index::{Took, record, summary};
use super::verify::{self, Candidate, Verdict};
use super::{families, multi_pair, order};

/// Проверка пачки кандидатов: окружение и кандидаты → итог по каждому (verify.rs на своём узле; тест решения 295 —
/// разборы на записанных ответах).
pub type Verifier = for<'a> fn(
    &'a Ctx,
    &'a [Candidate],
) -> Pin<Box<dyn Future<Output = GatewayResult<Vec<Verdict>>> + Send + 'a>>;

/// Проверка на своём узле (verify::verify). Ошибки: NotConfigured (dex.verify_batch).
pub fn on_node<'a>(
    ctx: &'a Ctx,
    cands: &'a [Candidate],
) -> Pin<Box<dyn Future<Output = GatewayResult<Vec<Verdict>>> + Send + 'a>> {
    Box::pin(verify::verify(ctx, cands))
}

/// take с заданной проверкой кандидатов (take — проверка на узле). Принимает: окружение, монеты, проверку.
/// Возвращает и ошибки: как take.
pub async fn take_with(
    ctx: &Ctx,
    only: Option<&HashSet<Address>>,
    check: Verifier,
) -> GatewayResult<Took> {
    let multi = multi_pair::multi_dex(ctx);
    ctx.index.with(|s| multi_pair::retune(s, &multi));
    let (mut failed, unconnected) = pass(ctx, only, None, check).await?;
    if only.is_none() {
        summary(ctx, unconnected).await;
    }
    let added = ctx.index.with(multi_pair::expand);
    let mut extra: HashSet<Address> = HashSet::new();
    if !added.is_empty() {
        extra = ctx.index.with(|s| {
            s.multi
                .iter()
                .filter(|(_, l)| {
                    l.iter().any(|p| added.contains(&multi_pair::dir_key(&s.multi_dex, &p.dex_id, &p.pool, p.base, p.quote)))
                })
                .map(|(a, _)| *a)
                .collect()
        });
        failed += pass(ctx, None, Some(&added), check).await?.0;
    }
    let found = ctx.index.with(|s| {
        s.taken
            .values()
            .filter(|f| only.is_none_or(|o| o.contains(&f.coin) || extra.contains(&f.coin)))
            .cloned()
            .collect()
    });
    Ok(Took {
        found: order::order(ctx, found),
        failed,
    })
}

/// Один проход проверки: строки справочника (монеты `only` или все; `keys` — только строки с этими ключами) →
/// кандидаты → проверка на узле → итог. Ключ строки — multi_pair::dir_key (у DEX вне режима — строка пула).
/// Возвращает: (непроверено из-за узла, не подключённые DEX). Ошибки: NotConfigured (dex.verify_batch).
async fn pass(
    ctx: &Ctx,
    only: Option<&HashSet<Address>>,
    keys: Option<&HashSet<String>>,
    check: Verifier,
) -> GatewayResult<(usize, BTreeSet<String>)> {
    let multi = ctx.index.with(|s| s.multi_dex.clone());
    let mut cands = Vec::new();
    let mut unconnected = BTreeSet::new();
    let mut seen = HashSet::new();
    let mut status = Vec::new();
    for (token, p) in ctx.index.relevant() {
        let quote = if p.base == token { p.quote } else { p.base };
        let key = multi_pair::dir_key(&multi, &p.dex_id, &p.pool, token, quote);
        if only.is_some_and(|o| !o.contains(&token))
            || keys.is_some_and(|k| !k.contains(&key))
            || !seen.insert(key.clone())
        {
            continue;
        }
        match families::classify(ctx, &p.dex_id) {
            Err(why) => {
                unconnected.insert(p.dex_id.clone());
                status.push((key, DirectoryStatus::NotConnected(why)));
            }
            Ok(family) => cands.push(Candidate {
                family,
                pool: p.pool,
                coin: token,
                quote,
            }),
        }
    }
    let verdicts = check(ctx, &cands).await?;
    ctx.index.with(|s| {
        for (key, st) in status {
            s.taken.remove(&key);
            s.status.insert(key, st);
        }
    });
    let failed = record(ctx, &cands, verdicts).await;
    Ok((failed, unconnected))
}

