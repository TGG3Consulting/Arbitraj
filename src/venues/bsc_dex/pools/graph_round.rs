//! Файл: venues/bsc_dex/pools/graph_round.rs
//! Что делает: решение 308 (задача t137) — один НАКОПИТЕЛЬНЫЙ круг фонового владельца графа: пулы, изменённые с прошлого
//! круга (graph_changes.rs drain), плюс зависимые по курсу встречного токена (graph_work.rs expand) оцениваются заново
//! (graph_eval.rs) и применяются к карте; новый Graph собирается из карты (топ пересобирается только у затронутых пар).
//! Работа — чистый CPU; вызывающий (graph_owner.rs) ведёт её в потоке блокирующих задач (spawn_blocking), не в рабочих
//! потоках tokio. Обхода всех пулов реестра нет — только изменённые и зависимые.
//! Кто вызывает: graph_owner.rs.
//! От чего зависит: graph_work.rs, graph_eval.rs, graph.rs (Graph), registry.rs, tokens.rs, ctx.rs.
//! Настройки: dex.route_max_pools_per_edge (значение даёт вызывающий); через книгу глубины — dex.depth_* и
//! dex.min_pool_liquidity_usd [bsc_dex].

use std::collections::HashSet;

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::graph::Graph;
use super::graph_eval;
use super::graph_work::Work;
use super::registry::Registry;

/// Итог круга.
pub struct RoundOut {
    pub graph: Graph,
    /// Оценённые пулы (изменённые и зависимые) — для набора «изменилось во время полной сборки» (graph_full.rs).
    pub evaluated: HashSet<PoolRef>,
    /// Пар с пересобранным топом.
    pub pairs: usize,
    /// Из оценённых — изменённых в реестре (остальные — зависимые по курсу встречного токена; ревью №1 R).
    pub changed: usize,
}

/// Накопительный круг. Принимает: окружение, реестр, токены, карту, изменённые пулы, dex.route_max_pools_per_edge,
/// признак «работа жива». Возвращает: новый граф и счётчики. Ошибки: (изменённые пулы — вызывающий вернёт их в доводку
/// или пересборку, ревью №2 E; текст): настройки глубины не заданы или неверны, работа отменена. Карта при ошибке не
/// меняется.
pub fn round(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    w: &mut Work,
    refs: HashSet<PoolRef>,
    per_edge: usize,
    alive: &dyn Fn() -> bool,
) -> Result<RoundOut, (HashSet<PoolRef>, String)> {
    let changed = refs.len();
    let mut all = HashSet::new();
    w.expand(reg, &refs, &mut all);
    let evals = match graph_eval::evaluate(ctx, reg, tokens, w.basis.usdt_dec, &all, alive) {
        Ok(v) => v,
        Err(why) => return Err((all, why)),
    };
    for (r, rec) in evals {
        w.apply(r, rec);
    }
    let (graph, pairs) = w.publish(per_edge);
    Ok(RoundOut {
        graph,
        evaluated: all,
        pairs,
        changed,
    })
}
