//! Файл: venues/bsc_dex/pools/graph_full.rs
//! Что делает: решение 308 (задача t137) — ПОЛНАЯ пересборка карты графа с нуля и самопроверка (одна операция, п. 11
//! проекта docs/notes/graph-snapshot-design-2026-11-16.md):
//! - build — каждый пул снимка реестра оценивается той же функцией, что накопительный круг (graph_eval.rs). Идёт в
//!   отдельном потоке блокирующих задач ПАРАЛЛЕЛЬНО накопительным кругам: читатели и круги её не ждут;
//! - reconcile — доводка после сборки: пулы, изменённые за время сборки (набор since от кругов и остаток drain),
//!   раскрываются по зависимостям курса обеих карт, оцениваются ОДИН раз и применяются к обеим картам — так обе карты
//!   стоят на одной версии реестра; затем сравнение множествами (graph_diff.rs). Действующей становится собранная с нуля
//!   (самоисцеление) — при расхождении и без него; торговля по графу не останавливается.
//!
//! Кто вызывает: graph_owner.rs.
//! От чего зависит: graph_work.rs, graph_eval.rs, graph_diff.rs, graph_snap.rs (Basis), depth_book.rs (prep), registry.rs,
//! tokens.rs, ctx.rs.
//! Настройки: dex.route_max_pools_per_edge (даёт вызывающий); через книгу глубины — dex.depth_*, dex.min_pool_liquidity_usd.

use std::collections::HashSet;

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::depth_book::Book;
use super::graph::Graph;
use super::graph_diff;
use super::graph_eval;
use super::graph_snap::Basis;
use super::graph_work::Work;
use super::registry::Registry;

/// Доводка не удалась: накопительная карта (остаётся действующей), пулы, которые не применены (вернуть в набор
/// изменений — ревью №1 N), причина.
pub type Unapplied = (Option<Work>, HashSet<PoolRef>, String);

/// Итог доводки.
pub struct Reconciled {
    /// Карта, собранная с нуля и доведённая до текущей версии реестра — новая действующая.
    pub work: Work,
    pub graph: Graph,
    /// Отличия накопительной карты от собранной с нуля; None — сравнивать было не с чем.
    pub diff: Option<Vec<String>>,
    /// Пулов оценено при доводке.
    pub evaluated: usize,
}

/// Сборка карты с нуля по снимку реестра. Принимает: окружение, реестр, токены, basis, признак «работа жива».
/// Возвращает: карту и сколько пулов было в снимке реестра. Ошибки (текст): настройки глубины не заданы или неверны;
/// работа отменена.
pub fn build(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    basis: Basis,
    alive: &dyn Fn() -> bool,
) -> Result<(Work, usize), String> {
    let prep = Book::prep(ctx, tokens)?;
    let mut w = Work::new(basis);
    let snap = reg.snapshot();
    let n = snap.len();
    for e in snap {
        // Ревью №2 (B): брошенная или заменённая сборка останавливается на ближайшем пуле, а не досчитывает впустую.
        if !alive() {
            return Err(graph_eval::CANCELLED.to_string());
        }
        let rec = graph_eval::eval(ctx, reg, tokens, w.basis.usdt_dec, &prep, &e);
        w.apply(e.pool.reference.clone(), Some(rec));
    }
    Ok((w, n))
}

/// Доводка и сверка (см. шапку). Принимает: окружение, реестр, токены, накопительную карту (None — нет или другие
/// настройки), карту с нуля, пулы, изменённые за время сборки, dex.route_max_pools_per_edge, признак «работа жива».
/// Возвращает: Reconciled.
/// Ошибки: настройки глубины не заданы или неверны — с возвратом накопительной карты (она остаётся действующей); за
/// Box — карта велика (clippy::result_large_err).
#[allow(clippy::too_many_arguments)]
pub fn reconcile(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    inc: Option<Work>,
    mut full: Work,
    refs: HashSet<PoolRef>,
    per_edge: usize,
    alive: &dyn Fn() -> bool,
) -> Result<Reconciled, Box<Unapplied>> {
    let mut inc = inc;
    let mut all = HashSet::new();
    full.expand(reg, &refs, &mut all);
    if let Some(i) = &inc {
        i.expand(reg, &refs, &mut all);
    }
    let evals = match graph_eval::evaluate(ctx, reg, tokens, full.basis.usdt_dec, &all, alive) {
        Ok(v) => v,
        Err(why) => return Err(Box::new((inc, all, why))),
    };
    for (r, rec) in evals {
        if let Some(i) = inc.as_mut() {
            i.apply(r.clone(), rec.clone());
        }
        full.apply(r, rec);
    }
    let (graph, _) = full.publish(per_edge);
    let diff = inc.map(|mut i| {
        i.refresh(per_edge);
        graph_diff::diff(&i.picture(), &full.picture())
    });
    Ok(Reconciled {
        work: full,
        graph,
        diff,
        evaluated: all.len(),
    })
}
