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
//! Кто вызывает: quote/graph_route.rs (cached), quote/cycle_scan.rs (build).
//! От чего зависит: pools/registry.rs, pools/depth_book.rs, tokens.rs, ctx.rs.
//! Настройки: dex.route_max_pools_per_edge [bsc_dex]; через depth_book — dex.min_pool_liquidity_usd,
//! dex.depth_band_pct, dex.depth_hysteresis_pct [bsc_dex].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::depth_book;
use super::registry::{Entry, Registry};

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

/// На чём построен граф (решение 129): версия реестра, блок головы, dex.route_max_pools_per_edge, настройки глубины
/// (полоса, порог, гистерезис — текстом), USDT и его decimals.
#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    version: u64,
    head: u64,
    settings: Vec<String>,
    usdt: Option<Address>,
    usdt_dec: u8,
}

/// Кэш последнего графа (решение 129). Один на реестр (pools/registry.rs). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Cache(Arc<Mutex<Option<Built>>>);

/// Построенный граф и на чём он построен.
type Built = (Stamp, Arc<Graph>);

/// Граф текущего состояния: из кэша, если с его построения не менялись реестр, блок головы и настройки графа, иначе
/// строится заново (build) и кладётся в кэш. Принимает: окружение, реестр, токены, знаки USDT, номер блока головы.
/// Возвращает: граф. Ошибки: как у build.
pub fn cached(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt_dec: u8,
    head: u64,
) -> GatewayResult<Arc<Graph>> {
    // Версия — ДО чтения реестра: запись во время построения даст новую версию, и следующий вызов перестроит граф.
    let version = reg.version();
    // Значения как прочитаны (не задана — пусто; build сам скажет, какой нет).
    let dec = |n: &str| {
        ctx.dec(n)
            .map(|d| d.normalize().to_string())
            .unwrap_or_default()
    };
    let stamp = Stamp {
        version,
        head,
        settings: vec![
            ctx.int("dex.route_max_pools_per_edge")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            dec("dex.depth_band_pct"),
            dec("dex.min_pool_liquidity_usd"),
            dec("dex.depth_hysteresis_pct"),
        ],
        usdt: tokens.usdt(ctx).ok(),
        usdt_dec,
    };
    let slot = &reg.graph_cache().0;
    if let Some((st, g)) = slot.lock().unwrap_or_else(|p| p.into_inner()).as_ref()
        && *st == stamp
    {
        return Ok(g.clone());
    }
    let g = Arc::new(build(ctx, reg, tokens, usdt_dec)?);
    let mut m = slot.lock().unwrap_or_else(|p| p.into_inner());
    // Не затираем более новый граф, построенный параллельно.
    if m.as_ref().is_none_or(|(st, _)| st.version <= version) {
        *m = Some((stamp, g.clone()));
    }
    Ok(g)
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
        // graphable, не tradable (решение 82): V2-форк входит в граф для поиска/сигналов, хотя авто-сделкой не
        // исполняется (допуск к сделке — quote/graph_route::executable). Исключены недостоверные, спящие и не-graphable.
        if !e.valid || e.asleep || !e.family.graphable {
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
