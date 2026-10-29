//! Файл: venues/bsc_dex/pools/graph.rs
//! Что делает: граф токенов в ОЗУ по всем отслеживаемым пулам (решение 80, бэклог шаг 1 — «модель графа»). Узлы —
//! токены, рёбра — пулы; строится снимком из реестра (pools/registry.rs) на момент вызова. Состояние ребра для
//! математики — это сама запись реестра (Entry), которую события пулов уже обновляют точечно (решение 83), поэтому
//! свой кэш состояния граф не держит — каждый снимок берёт живые записи. Отсекает заведомо непригодные рёбра:
//! недостоверные, спящие, не торгуемых DEX и мелкие по обеим сторонам (решение 73). На пару токенов оставляет не
//! больше dex.route_max_pools_per_edge самых глубоких рёбер, чтобы держать ширину будущего перебора путей.
//! Допуск ребра к КОТИРОВКЕ/СДЕЛКЕ (сверка с квотером, крючок, газ) проверяет quote/select.rs при оценке пути —
//! граф даёт только достижимость и ликвидность.
//! Кто вызывает: перечисление путей графа (решение 80, следующий шаг) — quote/*.
//! От чего зависит: pools/registry.rs, pools/depth_book.rs, tokens.rs, ctx.rs.
//! Настройки: dex.route_max_pools_per_edge [bsc_dex]; через depth_book — dex.min_pool_liquidity_usd,
//! dex.depth_band_pct, dex.depth_hysteresis_pct [bsc_dex].

use std::collections::HashMap;

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::depth_book;
use super::registry::{Entry, Registry};

/// Ребро графа: пул между токенами `a` и `b` (запись реестра со всем состоянием для математики).
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
    edges: Vec<Edge>,
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
    let mut by_pair: HashMap<(Address, Address), Vec<(Decimal, Entry)>> = HashMap::new();
    for e in reg.all() {
        if !e.valid || e.asleep || !e.family.tradable {
            continue;
        }
        // Глубина на живом состоянии; не посчиталась (устаревший блок, нет курса промежуточной) — ребро пропускаем.
        let Ok(d) = reg.depth().depth(ctx, reg, tokens, &e, usdt_dec) else {
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
            g.edges.push(Edge { entry: e, a, b });
            g.adj.entry(a).or_default().push(i);
            g.adj.entry(b).or_default().push(i);
        }
    }
    Ok(g)
}

/// Ключ неориентированной пары: концы по возрастанию адреса. Ошибок нет.
fn pair_key(x: Address, y: Address) -> (Address, Address) {
    if x <= y { (x, y) } else { (y, x) }
}
