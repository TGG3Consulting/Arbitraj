//! Файл: venues/bsc_dex/quote/route_paths.rs
//! Что делает: перечисление путей обмена по графу токенов (решение 80, бэклог шаг 2). От `from` (USDT) к `to` (монета)
//! поиском в глубину (DFS): пути длиной 1..=max_hops рёбер; внутренние узлы — только из набора разрешённых
//! промежуточных монет (решение 52); без повторного захода в токен — значит без циклов и без возврата в старт
//! (решение 80 §6: для направления BingX ↔ DEX самостоятельных циклов с возвратом в стартовый токен не строим).
//! Ширина ограничена max_paths, чтобы держать скорость. Параллельные пулы одной пары дают отдельные пути
//! (их потом делит сплит, шаг 80.4).
//! Путь нейтрален по стороне: порядок рёбер USDT→монета; покупка идёт так, продажа — обратным обходом (route_calc.rs).
//! Оценку пути (математику, затраты, amountOutMin) и допуск каждого пула (quote/select.rs) делает следующий шаг.
//!
//! Решение 85 (направление DEX ↔ DEX): здесь же — `enumerate_cycles` — перечисление ЗАМКНУТЫХ циклов (старт = финиш =
//! базовая монета кошелька) тем же механизмом DFS, но с возвратом в стартовый токен. Это второй режим того же
//! перебора: исходный `enumerate` (пути USDT ↔ монета, без возврата в старт) не трогается. Цикл — ≥ 2 хопов, один пул
//! в цикле не используется дважды, внутренние токены не повторяются. Оба обхода цикла (base→A→B→base и
//! base→B→A→base) получаются отдельными путями (разные первые рёбра) и считаются независимо (решение 85).
//! Решение 295: один многомонетный пул (PoolRef::site) — не больше одного шага пути и цикла.
//! Кто вызывает: граф-маршрутизатор (решение 80, шаг 80.4) — quote/graph_route.rs; поиск циклов DEX ↔ DEX
//! (решение 85) — quote/cycle_scan.rs.
//! Решение 410 (t195): стек перебора — ссылки на рёбра (quote/route_hops.rs), ребро копируется только в выданный путь.
//! От чего зависит: pools/graph.rs, quote/route_hops.rs, fast_hash.rs (решение 410); настройки читает вызывающий (limits / cycle_scan::limits).
//! Настройки: dex.route_max_hops, dex.route_max_paths [bsc_dex] (через limits); пределы циклов — dex_dex.max_cycle_len,
//! dex_dex.max_cycles [bsc_dex] (через quote/cycle_scan.rs).

use std::collections::HashSet;

use alloy_primitives::Address;

use crate::core::types::PoolRef;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::fast_hash::FastSet;
use super::super::pools::graph::{Edge, Graph};
use super::route_hops::{self, Hop};

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
    // Решение 410 (t195): посещённые токены и пулы — хеш foldhash (fast_hash.rs), только «есть ли».
    let mut visited: FastSet<Address> = FastSet::default();
    visited.insert(from);
    // Решение 410 (t195): стек — ссылки на рёбра (route_hops.rs), ребро копируется только в выданный путь.
    let mut steps: Vec<Hop<'_>> = Vec::new();
    let mut sites: FastSet<PoolRef> = FastSet::default();
    dfs(
        graph,
        from,
        to,
        allowed,
        max_hops,
        max_paths,
        &mut visited,
        &mut sites,
        &mut steps,
        &mut out,
    );
    out
}

/// Обход в глубину: из `node` (пройдено `steps.len()` рёбер) тянем рёбра к цели или к разрешённой промежуточной.
/// Решение 295: `sites` — многомонетные пулы (PoolRef::Multi), уже пройденные путём: другая пара того же пула в
/// этот путь не идёт (одна площадка — один шаг). Пулы с иной ссылкой в `sites` не попадают (у парного пула повтор
/// невозможен — нет повтора токена). Ошибок нет.
#[allow(clippy::too_many_arguments)]
fn dfs<'g>(
    graph: &'g Graph,
    node: Address,
    to: Address,
    allowed: &HashSet<Address>,
    max_hops: usize,
    max_paths: usize,
    visited: &mut FastSet<Address>,
    sites: &mut FastSet<PoolRef>,
    steps: &mut Vec<Hop<'g>>,
    out: &mut Vec<RoutePath>,
) {
    for edge in graph.edges_from(node) {
        if out.len() >= max_paths {
            return;
        }
        let multi = [edge.entry.shared_market(), edge.entry.second_liquidity_site()];
        if multi.iter().flatten().any(|s| sites.contains(s)) {
            continue;
        }
        let next = edge.other(node);
        // Решение 410 (t195): шаг — ссылка на ребро; копия ребра — только в выданный путь (route_hops::path).
        let step: Hop<'g> = (edge, node, next);
        if next == to {
            // Путь замкнулся на цель: длиной steps.len() + 1 хопов (в пределах max_hops — цикл идёт только пока есть запас).
            out.push(route_hops::path(steps, step));
        } else if steps.len() + 1 < max_hops && allowed.contains(&next) && !visited.contains(&next)
        {
            // Внутренний узел — только разрешённая промежуточная, без повторного захода (нет циклов, нет возврата в старт).
            visited.insert(next);
            for s in multi.iter().flatten() {
                sites.insert(s.clone());
            }
            steps.push(step);
            dfs(
                graph, next, to, allowed, max_hops, max_paths, visited, sites, steps, out,
            );
            steps.pop();
            for s in multi.iter().flatten() {
                sites.remove(s);
            }
            visited.remove(&next);
        }
    }
}

/// Перечисление замкнутых циклов от `base` обратно в `base` (решение 85, направление DEX ↔ DEX). Принимает: граф,
/// базовую монету (старт = финиш), предел длины цикла в хопах (рёбрах) и предел числа найденных циклов.
/// Цикл — путь длиной 2..=max_cycle_len рёбер, начинающийся и заканчивающийся в `base`; один пул в цикле не
/// используется дважды, внутренние токены не повторяются (кроме финального возврата в base). Разрешённые
/// промежуточные (решение 52) к циклу НЕ применяются — внутренним узлом может быть любой токен графа (решение 85:
/// нужна только ликвидность пулов маршрута). Возвращает: циклы (может быть пусто). Ошибок нет: пределы проверяет
/// вызывающий (quote/cycle_scan.rs).
pub fn enumerate_cycles(
    graph: &Graph,
    base: Address,
    max_cycle_len: usize,
    max_cycles: usize,
) -> Vec<RoutePath> {
    let mut out: Vec<RoutePath> = Vec::new();
    // Цикл — минимум 2 хопа (base → X → base разными пулами); длиной 1 замкнуться не на чем (нет ребра base–base).
    if max_cycle_len < 2 || max_cycles == 0 {
        return out;
    }
    // Решение 410 (t195): хеш foldhash (fast_hash.rs), только «есть ли».
    let mut visited: FastSet<Address> = FastSet::default();
    visited.insert(base);
    let mut used: FastSet<PoolRef> = FastSet::default();
    // Решение 410 (t195): стек — ссылки на рёбра (route_hops.rs).
    let mut steps: Vec<Hop<'_>> = Vec::new();
    dfs_cycle(
        graph,
        base,
        base,
        max_cycle_len,
        max_cycles,
        &mut visited,
        &mut used,
        &mut steps,
        &mut out,
    );
    out
}

/// Обход в глубину для циклов: из `node` тянем рёбра обратно к `base` (замыкание) или к ещё не посещённому токену.
/// Один пул (ребро по PoolRef) в одном цикле — не дважды. Ошибок нет.
#[allow(clippy::too_many_arguments)]
fn dfs_cycle<'g>(
    graph: &'g Graph,
    node: Address,
    base: Address,
    max_cycle_len: usize,
    max_cycles: usize,
    visited: &mut FastSet<Address>,
    used: &mut FastSet<PoolRef>,
    steps: &mut Vec<Hop<'g>>,
    out: &mut Vec<RoutePath>,
) {
    for edge in graph.edges_from(node) {
        if out.len() >= max_cycles {
            return;
        }
        // Один и тот же пул в цикле не используем дважды (атомарный бандл свопа по пулу — один раз, решение 85/84).
        // Решение 295: по физическому пулу (site) — разные пары одного многомонетного пула тоже «тот же пул»; у прочих
        // ссылок site() = сама ссылка.
        let sites = [Some(edge.entry.liquidity_site()), edge.entry.second_liquidity_site()];
        if sites.iter().flatten().any(|s|used.contains(s)) {
            continue;
        }
        let next = edge.other(node);
        // Решение 410 (t195): шаг — ссылка на ребро; копия — только в выданный цикл.
        let step: Hop<'g> = (edge, node, next);
        if next == base {
            // Цикл замкнулся на базовую монету: длиной steps.len() + 1 хопов (≥ 2 и в пределах max_cycle_len).
            if steps.len() + 1 >= 2 {
                out.push(route_hops::path(steps, step));
            }
        } else if steps.len() + 1 < max_cycle_len && !visited.contains(&next) {
            // Внутренний токен — любой, без повторного захода; остаётся запас хотя бы на одно замыкающее ребро.
            visited.insert(next);
            used.extend(sites.iter().flatten().cloned());
            steps.push(step);
            dfs_cycle(
                graph,
                next,
                base,
                max_cycle_len,
                max_cycles,
                visited,
                used,
                steps,
                out,
            );
            steps.pop();
            for site in sites.iter().flatten() {used.remove(site);}
            visited.remove(&next);
        }
    }
}

/// Физический пул ребра, если это пара многомонетного пула (решение 295, PoolRef::Multi); иначе None — ребро в запрет
/// «одна площадка — один шаг» путей не входит (route_best.rs — тот же запрет). Ошибок нет.
pub fn multi_site(r: &PoolRef) -> Option<PoolRef> {
    matches!(r, PoolRef::Multi { .. }).then(|| r.site())
}
