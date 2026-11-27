//! Файл: venues/bsc_dex/quote/route_hops.rs
//! Что делает: стек шагов перебора путей и циклов по ССЫЛКАМ на рёбра графа (решение 410, задача t195) и сборка
//! готового пути из него. Раньше перебор (route_paths.rs dfs / dfs_cycle, route_best.rs Walk::dfs) копировал ребро
//! (с ним — запись пула Entry: Pool, три строки, состояние) на КАЖДОЕ просмотренное ребро, даже отброшенное
//! (не та цель, не разрешённая промежуточная), на каждый шаг вглубь и ещё раз при выдаче пути (копия стека). Теперь
//! ребро копируется только в выданный путь — ровно те же шаги в том же порядке, что раньше.
//! Кто вызывает: quote/route_paths.rs (enumerate, enumerate_cycles), quote/route_best.rs (enumerate).
//! От чего зависит: pools/graph.rs (Edge), quote/route_paths.rs (Step, RoutePath).
//! Настройки: нет.

use alloy_primitives::Address;

use super::super::pools::graph::Edge;
use super::route_paths::{RoutePath, Step};

/// Шаг перебора без копии ребра: ребро графа, токен входа, токен выхода.
pub type Hop<'g> = (&'g Edge, Address, Address);

/// Multi-role markets share state with standalone Curve pools even when that earlier
/// ordinary contract did not itself declare a second liquidity site.
pub(super) fn physical_overlap(
    edge: &Edge,
    steps: &[Hop<'_>],
    sites: &super::super::fast_hash::FastSet<crate::core::types::PoolRef>,
) -> bool {
    if !sites.is_empty() && sites.contains(&edge.entry.pool.reference) {
        return true;
    }
    (edge.entry.family.variant.euler() || edge.entry.family.variant == super::super::pools::family_variant::Variant::YieldBasis)
        && steps
            .iter()
            .any(|(prior, _, _)| edge.entry.shares_liquidity(&prior.entry))
}

/// Что делает: путь из стека шагов перебора и замыкающего шага — копия каждого ребра в шаг пути, порядок стека.
/// Принимает: пройденные шаги, последний шаг. Возвращает: путь (те же шаги, что раньше давала копия стека Step).
/// Ошибок нет.
pub fn path(steps: &[Hop<'_>], last: Hop<'_>) -> RoutePath {
    let steps = steps
        .iter()
        .chain(std::iter::once(&last))
        .map(|&(edge, token_in, token_out)| Step {
            edge: edge.clone(),
            token_in,
            token_out,
        })
        .collect();
    RoutePath { steps }
}
