//! Файл: venues/bsc_dex/quote/route_paths.rs
//! Что делает: перечисление путей обмена по графу токенов (решение 80, бэклог шаг 2). От `from` (USDT) к `to` (монета)
//! поиском в глубину (DFS): пути длиной 1..=max_hops рёбер; внутренние узлы — только из набора разрешённых
//! промежуточных монет (решение 52); без повторного захода в токен — значит без циклов и без возврата в старт
//! (решение 80 §6: самостоятельных циклов с возвратом в стартовый токен не строим). Ширина ограничена max_paths,
//! чтобы держать скорость. Параллельные пулы одной пары дают отдельные пути (их потом делит сплит, шаг 80.4).
//! Путь нейтрален по стороне: порядок рёбер USDT→монета; покупка идёт так, продажа — обратным обходом (route_calc.rs).
//! Оценку пути (математику, затраты, amountOutMin) и допуск каждого пула (quote/select.rs) делает следующий шаг.
//! Кто вызывает: граф-маршрутизатор (решение 80, шаг 80.4) — quote/graph_route.rs.
//! От чего зависит: pools/graph.rs; настройки читает вызывающий (limits).
//! Настройки: dex.route_max_hops, dex.route_max_paths [bsc_dex] (через limits).

use std::collections::HashSet;

use alloy_primitives::Address;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::pools::graph::{Edge, Graph};

/// Один шаг пути: пул (снимок ребра графа) и токены входа/выхода в порядке исполнения.
#[derive(Clone)]
pub struct Step {
    pub edge: Edge,
    pub token_in: Address,
    pub token_out: Address,
}

/// Путь обмена: шаги в порядке от `from` к `to`. Длина — число рёбер (хопов).
#[derive(Clone, Default)]
pub struct RoutePath {
    pub steps: Vec<Step>,
}

impl RoutePath {
    /// Число хопов. Ошибок нет.
    pub fn hops(&self) -> usize {
        self.steps.len()
    }
}

/// Пределы перебора из настроек. Ошибки: NotConfigured (не заданы или вне допустимого).
pub fn limits(ctx: &Ctx) -> GatewayResult<(usize, usize)> {
    let max_hops = usize::try_from(ctx.int("dex.route_max_hops")?).unwrap_or(0);
    let max_paths = usize::try_from(ctx.int("dex.route_max_paths")?).unwrap_or(0);
    if max_hops == 0 {
        return Err(ctx.not_set(
            "dex.route_max_hops",
            "dex.route_max_hops должна быть больше нуля",
        ));
    }
    if max_paths == 0 {
        return Err(ctx.not_set(
            "dex.route_max_paths",
            "dex.route_max_paths должна быть больше нуля",
        ));
    }
    Ok((max_hops, max_paths))
}

/// Перечисление путей `from` → `to`. Принимает: граф, старт (USDT), цель (монета), разрешённые промежуточные
/// (внутренние узлы пути), предел хопов и предел числа путей. Возвращает: пути (может быть пусто — путей нет).
/// Ошибок нет: пределы проверяет `limits`.
pub fn enumerate(
    graph: &Graph,
    from: Address,
    to: Address,
    allowed: &HashSet<Address>,
    max_hops: usize,
    max_paths: usize,
) -> Vec<RoutePath> {
    let mut out: Vec<RoutePath> = Vec::new();
    if max_hops == 0 || max_paths == 0 || from == to {
        return out;
    }
    let mut visited: HashSet<Address> = HashSet::from([from]);
    let mut steps: Vec<Step> = Vec::new();
    dfs(
        graph,
        from,
        to,
        allowed,
        max_hops,
        max_paths,
        &mut visited,
        &mut steps,
        &mut out,
    );
    out
}

/// Обход в глубину: из `node` (пройдено `steps.len()` рёбер) тянем рёбра к цели или к разрешённой промежуточной.
/// Ошибок нет.
#[allow(clippy::too_many_arguments)]
fn dfs(
    graph: &Graph,
    node: Address,
    to: Address,
    allowed: &HashSet<Address>,
    max_hops: usize,
    max_paths: usize,
    visited: &mut HashSet<Address>,
    steps: &mut Vec<Step>,
    out: &mut Vec<RoutePath>,
) {
    for edge in graph.edges_from(node) {
        if out.len() >= max_paths {
            return;
        }
        let next = edge.other(node);
        let step = Step {
            edge: edge.clone(),
            token_in: node,
            token_out: next,
        };
        if next == to {
            // Путь замкнулся на цель: длиной steps.len() + 1 хопов (в пределах max_hops — цикл идёт только пока есть запас).
            steps.push(step);
            out.push(RoutePath {
                steps: steps.clone(),
            });
            steps.pop();
        } else if steps.len() + 1 < max_hops && allowed.contains(&next) && !visited.contains(&next)
        {
            // Внутренний узел — только разрешённая промежуточная, без повторного захода (нет циклов, нет возврата в старт).
            visited.insert(next);
            steps.push(step);
            dfs(
                graph, next, to, allowed, max_hops, max_paths, visited, steps, out,
            );
            steps.pop();
            visited.remove(&next);
        }
    }
}
