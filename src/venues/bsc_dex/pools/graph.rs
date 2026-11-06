//! Файл: venues/bsc_dex/pools/graph.rs
//! Что делает: граф токенов в ОЗУ по всем отслеживаемым пулам (решение 80, бэклог шаг 1 — «модель графа»). Узлы —
//! токены, рёбра — пулы; строится снимком из реестра (pools/registry.rs) на момент вызова build. Состояние ребра
//! (Edge.entry) — КОПИЯ записи реестра на этот момент (снимок), а не живая ссылка: позже события пулов меняют реестр,
//! но снимок не меняется. Поэтому потребитель перед сделкой ОБЯЗАН перечитать/верифицировать состояние выбранных
//! пулов точечным Multicall3 (решение 83, exec/precheck.rs): граф даёт достижимость и ликвидность на момент снимка,
//! а не гарантию актуальности к исполнению. Отсекает заведомо непригодные рёбра: недостоверные, спящие, не входящие
//! в граф (family.graphable=false — например V3-форк) и мелкие по обеим сторонам (не глубокие ни по одной — решение
//! 73, стороны независимы). V2-форк (graphable, но не tradable, решение 82) в граф входит. На пару токенов
//! оставляет не больше dex.route_max_pools_per_edge самых глубоких рёбер, чтобы держать ширину перебора путей.
//! Допуск ребра к КОТИРОВКЕ/СДЕЛКЕ (сверка с квотером, крючок, газ) проверяет quote/select.rs при оценке пути —
//! граф даёт только достижимость и ликвидность.
//! Решение 129: `cached` — граф строится один раз на состояние реестра (версия реестра), блок головы и настройки графа;
//! все котировки этого состояния (подбор объёма считает десятки объёмов на кандидата, кандидаты идут параллельно)
//! берут готовый снимок. Раньше каждая котировка DEX копировала весь реестр с картами тиков и считала ключ глубины
//! каждого пула — счёт в задаче tokio без отдачи управления. Результат тот же: граф зависит только от реестра,
//! настроек, токенов и (через устаревание) блока — всё это в ключе кэша.
//! Кэш графа (cached, один на состояние реестра) — pools/graph_cache.rs.
//! Кто вызывает: quote/graph_route.rs, quote/cycle_scan.rs (cached), graph_cache.rs (build).
//! От чего зависит: pools/registry.rs, pools/depth_book.rs, tokens.rs, ctx.rs.
//! Настройки: dex.route_max_pools_per_edge [bsc_dex]; через depth_book — dex.min_pool_liquidity_usd,
//! dex.depth_band_pct, dex.depth_hysteresis_pct [bsc_dex].

use std::collections::HashMap;
use std::sync::Arc;

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::depth_book;
use super::registry::{Entry, Registry};

pub use super::graph_cache::{Cache, PathsKey, Skeleton, cached};

/// Ребро графа: пул между токенами `a` и `b`. `entry` — КОПИЯ записи реестра на момент build (снимок, не живая
/// ссылка); перед сделкой состояние перечитывается точечным Multicall3 (решение 83).
#[derive(Clone)]
pub struct Edge {
    pub entry: Entry,
    pub a: Address,
    pub b: Address,
}

impl Edge {
    /// Другой конец ребра относительно `token`. Ошибок нет (token — один из концов ребра).
    pub fn other(&self, token: Address) -> Address {
        if token == self.a { self.b } else { self.a }
    }
}

/// Граф токенов — снимок реестра пулов. Узлы индексируются по токену, рёбра — по индексу в `edges`.
#[derive(Default)]
pub struct Graph {
    pub(super) edges: Vec<Edge>,
    /// Токен → индексы рёбер, у которых он один из концов.
    adj: HashMap<Address, Vec<usize>>,
}

impl Graph {
    /// Число рёбер графа. Ошибок нет.
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    /// Пуст ли граф. Ошибок нет.
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Рёбра, выходящие из токена (в любую сторону). Ошибок нет.
    pub fn edges_from(&self, token: Address) -> impl Iterator<Item = &Edge> {
        self.adj
            .get(&token)
            .into_iter()
            .flatten()
            .map(move |&i| &self.edges[i])
    }

    /// Рёбра между парой токенов (концы в любом порядке). Ошибок нет.
    pub fn between(&self, x: Address, y: Address) -> Vec<&Edge> {
        self.edges_from(x).filter(|e| e.other(x) == y).collect()
    }
}

/// Построение графа из реестра (решение 80). Берёт достоверные, не спящие пулы торгуемых DEX, глубокие хотя бы по
/// одной стороне (решение 73), и оставляет на пару токенов не больше dex.route_max_pools_per_edge самых глубоких.
/// Принимает: окружение, реестр, токены, знаки USDT. Возвращает: граф (может быть пустым — путей нет).
/// Ошибки: NotConfigured (не задана настройка глубины или dex.route_max_pools_per_edge).
pub fn build(ctx: &Ctx, reg: &Registry, tokens: &Tokens, usdt_dec: u8) -> GatewayResult<Graph> {
    if let Some(name) = depth_book::missing_setting(ctx) {
        return Err(ctx.not_set(&name, format!("граф маршрутов: не задана {name}")));
    }
    let max_per_edge =
        usize::try_from(ctx.int("dex.route_max_pools_per_edge")?).unwrap_or(usize::MAX);
    if max_per_edge == 0 {
        return Err(ctx.not_set(
            "dex.route_max_pools_per_edge",
            "dex.route_max_pools_per_edge должна быть больше нуля",
        ));
    }
    // Кандидаты по неориентированной паре токенов: (глубина для ранжирования, запись пула).
    // Решение 129: снимок записей без копирования (Arc); запись копируется только в оставленное ребро.
    let mut by_pair: HashMap<(Address, Address), Vec<Ranked>> = HashMap::new();
    // Решение 131: настройки глубины, USDT и отметки пулов «встречный — USDT» — один раз на построение, а не на пул.
    let Ok(prep) = depth_book::Book::prep(ctx, tokens) else {
        return Ok(Graph::default());
    };
    for e in reg.snapshot() {
        // graphable, не tradable (решение 82): V2-форк входит в граф для поиска/сигналов, хотя авто-сделкой не
        // исполняется (допуск к сделке — quote/graph_route::executable). Исключены недостоверные, спящие и не-graphable.
        if !e.valid || e.asleep || !e.family.graphable {
            continue;
        }
        // Глубина на живом состоянии; не посчиталась (устаревший блок, нет курса промежуточной) — ребро пропускаем.
        let Ok(d) = reg
            .depth()
            .depth_prep(ctx, reg, tokens, &e, usdt_dec, &prep)
        else {
            continue;
        };
        if !(d.deep_buy || d.deep_sell) {
            continue;
        }
        let rank = d.band.buy.max(d.band.sell);
        by_pair
            .entry(pair_key(e.coin, e.usdt))
            .or_default()
            .push((rank, e));
    }
    let mut g = Graph::default();
    for (_, mut list) in by_pair {
        list.sort_by_key(|x| std::cmp::Reverse(x.0)); // по убыванию глубины — оставить самые глубокие
        list.truncate(max_per_edge);
        for (_, e) in list {
            let (a, b) = (e.coin, e.usdt);
            let i = g.edges.len();
            g.edges.push(Edge {
                entry: (*e).clone(),
                a,
                b,
            });
            g.adj.entry(a).or_default().push(i);
            g.adj.entry(b).or_default().push(i);
        }
    }
    Ok(g)
}

/// Пул-кандидат ребра: глубина для ранжирования и запись.
type Ranked = (Decimal, Arc<Entry>);

/// Ключ неориентированной пары: концы по возрастанию адреса. Ошибок нет.
fn pair_key(x: Address, y: Address) -> (Address, Address) {
    if x <= y { (x, y) } else { (y, x) }
}
