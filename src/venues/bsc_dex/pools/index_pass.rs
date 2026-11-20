//! Файл: venues/bsc_dex/pools/index_pass.rs
//! Что делает: решение 295 — проход проверки справочника (index::take): режим dex.multi_pair (multi_pair::retune),
//! проход строк справочника с ключом строки multi_pair::dir_key (у DEX вне режима — строка пула, как до решения),
//! развёртка недостающих пар взятых многомонетных пулов (multi_pair::expand) и одна проверка новых строк в том же
//! вызове. Проверка кандидатов — параметром (на узле — verify::verify; тест — разборы на записанных ответах).
//! Решение 450 (T448-02): у id строки с dex.sub_dex пул проверяется видом своего ключа и видами дочерних ключей
//! (sub_dex.rs) — итог строки — первый вид, чья проверка пул подтвердила; ключ строки — по id индексатора.
//! Кто вызывает: pools/index.rs (take), tests/bsc_dex_multi_pair.rs (take_with).
//! От чего зависит: index.rs (Took, record_with, summary), verify.rs, sub_dex.rs, multi_pair.rs, order.rs, directory.
//! Настройки: через вызываемые — dex.verify_batch, dex.multi_pair, dex.sub_dex, dex.* [DEX].

use std::collections::{BTreeSet, HashSet};
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;

use alloy_primitives::Address;

use crate::core::types::DirectoryStatus;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::index::{Took, record_with, summary};
use super::verify::{self, Candidate, Verdict};
use super::{multi_pair, order, sub_dex};

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
    let mut rows: Vec<(String, Range<usize>)> = Vec::new();
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
        // Решение 450: вид своего ключа и подключённые дочерние ключи dex.sub_dex (без неё — один вид, как раньше).
        let (fams, why) = sub_dex::kinds(ctx, &p.dex_id);
        if fams.is_empty() {
            unconnected.insert(p.dex_id.clone());
            status.push((key, DirectoryStatus::NotConnected(why.join("; "))));
            continue;
        }
        let start = cands.len();
        cands.extend(fams.into_iter().map(|family| Candidate {
            family,
            pool: p.pool.clone(),
            coin: token,
            quote,
        }));
        rows.push((p.dex_id.clone(), start..cands.len()));
    }
    let verdicts = check(ctx, &cands).await?;
    ctx.index.with(|s| {
        for (key, st) in status {
            s.taken.remove(&key);
            s.status.insert(key, st);
        }
    });
    let (picked, ids, verdicts) = combine(&cands, rows, verdicts);
    let failed = record_with(ctx, &picked, Some(&ids), verdicts).await;
    Ok((failed, unconnected))
}

/// Решение 450: итог строки справочника по итогам её видов (sub_dex::pick). Принимает: кандидатов, строки (id строки,
/// диапазон её кандидатов), итоги по кандидатам. Возвращает: кандидата-итог строки (взят — его вид, иначе первый), id
/// строк и итоги — по строкам. Ошибок нет.
fn combine(
    cands: &[Candidate],
    rows: Vec<(String, Range<usize>)>,
    verdicts: Vec<Verdict>,
) -> (Vec<Candidate>, Vec<String>, Vec<Verdict>) {
    let mut it = verdicts.into_iter();
    let (mut picked, mut ids, mut out) = (Vec::new(), Vec::new(), Vec::new());
    for (id, r) in rows {
        let all: Vec<(&str, Verdict)> = cands[r.clone()]
            .iter()
            .map(|c| c.family.id)
            .zip(it.by_ref())
            .collect();
        if all.is_empty() {
            continue;
        }
        let (i, v) = sub_dex::pick(all);
        picked.push(cands[r.start + i].clone());
        ids.push(id);
        out.push(v);
    }
    (picked, ids, out)
}

