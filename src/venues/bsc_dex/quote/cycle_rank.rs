//! Файл: venues/bsc_dex/quote/cycle_rank.rs
//! Что делает: решение 404 — отбор N ЛУЧШИХ циклов DEX ↔ DEX на базовую монету вместо «первых N найденных» (было:
//! quote/route_paths.rs enumerate_cycles обрывал перебор на dex_dex.max_cycles циклах в порядке рёбер графа — порядок
//! HashMap, то есть случайные несколько монет за проход). Перебор — тот же поиск в глубину по снимку графа (те же
//! правила: старт = финиш = базовая, 2..=dex_dex.max_cycle_len хопов, физический пул в цикле один раз, внутренние
//! токены не повторяются), но цикл не копируется: путь держится индексами рёбер. У каждого цикла — «курс круга» =
//! произведение курсов рёбер с комиссией пула (quote/cycle_rate.rs: спот × (1 − комиссия), у Solidly стабильного и
//! StableSwap — котировка малой суммы). Курс > 1 — по текущим ценам круг возвращает больше вложенного до проскальзывания
//! и газа; это не доказательство и не отсев (строгий отсев — quote/bound_cycle.rs), только порядок: полный расчёт
//! получают dex_dex.max_cycles лучших.
//! Порядок полный и детерминированный (ревью t189, Н3 — мелкие пулы с уехавшей ценой не вытесняют настоящие циклы):
//! (1) все стороны пулов цикла глубокие по книге глубины (pools/depth_book.rs: глубина в полосе ≥
//! dex.min_pool_liquidity_usd, решение 73) — впереди; мелкая сторона по определению проекта не держит объём, её курс на
//! бесконечно малом входе на сумме сделки не исполним; (2) курс круга по убыванию; (3) глубина узкого места (USD) по
//! убыванию; (4) ключ цикла (байты пулов и токенов входа) по возрастанию. Итог не зависит от порядка рёбер графа, пока
//! предел dex_dex.rank_max_cycles не достигнут.
//! Нагрузка (ревью t189, Н5): последний шаг цикла берёт только рёбра «узел — база» по индексу (closers — один раз на базу),
//! а не все рёбра узла, — стоимость прохода ≈ числу циклов. Отбор — не больше 2·N кандидатов (сжатие до N), цикл ниже
//! порога N-го отбрасывается до выделения памяти. Замков в переборе нет: курсы и стороны пулов — память прохода
//! (quote/cycle_rate.rs Rates). Рёбра копируются только у N отобранных.
//! Кто вызывает: quote/cycle_scan.rs (scan); тесты.
//! От чего зависит: pools/graph.rs (edge_ids, edge), quote/cycle_rate.rs, quote/route_paths.rs (RoutePath, Step), ctx.rs.
//! Настройки: читает вызывающий — dex_dex.max_cycle_len, dex_dex.max_cycles, dex_dex.rank_max_cycles [bsc_dex];
//! через cycle_rate — dex.v2_fee_pct [вид пула V2].

use std::collections::HashMap;

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::pools::graph::Graph;
use super::cycle_rate::Rates;
use super::route_paths::{RoutePath, Step};

/// Итог отбора по одной базовой монете.
pub struct Ranked {
    /// Отобранные циклы, лучшие первыми.
    pub paths: Vec<RoutePath>,
    /// Курс круга каждого отобранного (тот же порядок).
    pub rates: Vec<Decimal>,
    /// Сколько циклов просмотрено перебором.
    pub seen: u64,
    /// Из них с курсом круга > 1.
    pub above_one: u64,
    /// Из них все стороны пулов глубокие.
    pub deep: u64,
    /// Из них глубокие с курсом круга > 1.
    pub deep_above_one: u64,
    /// Наибольший курс круга среди просмотренных.
    pub best: Option<Decimal>,
    /// Наибольший курс круга среди глубоких.
    pub best_deep: Option<Decimal>,
    /// Циклов без курса (у ребра нет цены или комиссии) — в отбор не идут.
    pub unrated: u64,
    /// Перебор остановлен пределом dex_dex.rank_max_cycles (отбор — из просмотренных).
    pub capped: bool,
}

/// Кандидат отбора: глубокий ли, курс, шаги (ребро, токен входа), глубина узкого места и ключ (при сжатии).
struct Cand {
    deep: bool,
    score: Decimal,
    hops: Vec<(usize, Address)>,
    depth: Option<Decimal>,
    key: Vec<u8>,
}

/// Состояние перебора одной базовой монеты.
struct Walk<'a> {
    settlement: Option<Address>,
    ctx: &'a Ctx,
    g: &'a Graph,
    rates: &'a mut Rates,
    base: Address,
    max_len: usize,
    top_n: usize,
    cap: Option<u64>,
    /// Соседи базы → рёбра к базе (замыкание последнего шага).
    closers: &'a HashMap<Address, Vec<usize>>,
    visited: Vec<Address>,
    used: Vec<PoolRef>,
    hops: Vec<(usize, Address)>,
    kept: Vec<Cand>,
    floor: Option<(bool, Decimal)>,
    out: Ranked,
}

/// Отбор top_n лучших циклов base → … → base. Принимает: окружение, снимок графа, память прохода, базовую, длину цикла
/// (≥ 2), число лучших (> 0), предел просмотра (None — без предела). Возвращает: Ranked. Ошибок нет: пределы проверяет
/// вызывающий (cycle_scan::limits).
#[allow(clippy::too_many_arguments)]
pub fn top(
    ctx: &Ctx,
    g: &Graph,
    rates: &mut Rates,
    base: Address,
    max_len: usize,
    top_n: usize,
    cap: Option<u64>,
) -> Ranked {
    top_settlement(ctx, g, rates, base, max_len, top_n, cap, None)
}
#[allow(clippy::too_many_arguments)]
pub fn top_settlement(
    ctx: &Ctx,
    g: &Graph,
    rates: &mut Rates,
    base: Address,
    max_len: usize,
    top_n: usize,
    cap: Option<u64>,
    settlement: Option<Address>,
) -> Ranked {
    let out = Ranked {
        paths: Vec::new(),
        rates: Vec::new(),
        seen: 0,
        above_one: 0,
        deep: 0,
        deep_above_one: 0,
        best: None,
        best_deep: None,
        unrated: 0,
        capped: false,
    };
    if max_len < 2 || top_n == 0 {
        return out;
    }
    let mut closers: HashMap<Address, Vec<usize>> = HashMap::new();
    for &i in g.edge_ids(base) {
        closers.entry(g.edge(i).other(base)).or_default().push(i);
    }
    let mut w = Walk {
        settlement,
        ctx,
        g,
        rates,
        base,
        max_len,
        top_n,
        cap,
        closers: &closers,
        visited: vec![base],
        used: Vec::new(),
        hops: Vec::new(),
        kept: Vec::new(),
        floor: None,
        out,
    };
    w.dfs(base);
    w.compact();
    let mut out = w.out;
    for c in &w.kept {
        let steps = c
            .hops
            .iter()
            .map(|&(i, token_in)| {
                let e = g.edge(i);
                Step {
                    edge: e.clone(),
                    token_in,
                    token_out: e.other(token_in),
                }
            })
            .collect();
        out.paths.push(RoutePath { steps });
        out.rates.push(c.score);
    }
    out
}

impl Walk<'_> {
    /// Обход в глубину (правила enumerate_cycles: пул по site один раз, токен один раз, замыкание в base). На последнем
    /// шаге — только рёбра к базе (closers): прочие рёбра узла там всё равно не продолжаются. Ошибок нет.
    fn dfs(&mut self, node: Address) {
        let (g, closers) = (self.g, self.closers);
        let last = self.hops.len() + 1 == self.max_len;
        let ids: &[usize] = if last {
            closers.get(&node).map(Vec::as_slice).unwrap_or_default()
        } else {
            g.edge_ids(node)
        };
        for &i in ids {
            if self.out.capped {
                return;
            }
            let e = g.edge(i);
            let site = e.entry.liquidity_site();
            if self.used.contains(&site) {
                continue;
            }
            let next = e.other(node);
            if next == self.base {
                if !self.hops.is_empty() {
                    self.hops.push((i, node));
                    self.close();
                    self.hops.pop();
                }
            } else if !last
                && self.settlement == Some(next)
                && self.visited.get(1) == Some(&next)
                && self.hops.len() >= 2
            {
                // Внутренний простой цикл вернулся к кассе. Теперь допустим только последний обмен в USDT.
                let first = &g.edge(self.hops[0].0).entry;
                for &exit in closers.get(&next).into_iter().flatten() {
                    if self.out.capped {
                        return;
                    }
                    let close = &g.edge(exit).entry;
                    let terminal = close.liquidity_site();
                    if terminal == site
                        || (self.used.contains(&terminal)
                            && (first.pool.reference != close.pool.reference
                                || !super::cycle_reuse::supported(first)))
                    {
                        continue;
                    }
                    self.hops.push((i, node));
                    self.hops.push((exit, next));
                    self.close();
                    self.hops.pop();
                    self.hops.pop();
                }
            } else if !last && !self.visited.contains(&next) {
                self.visited.push(next);
                self.used.push(site);
                self.hops.push((i, node));
                self.dfs(next);
                self.hops.pop();
                self.used.pop();
                self.visited.pop();
            }
        }
    }

    /// Замкнутый цикл в self.hops: учёт, курс, глубокий ли, отбор. Ошибок нет.
    fn close(&mut self) {
        if self.cap.is_some_and(|c| self.out.seen >= c) {
            self.out.capped = true;
            return;
        }
        self.out.seen += 1;
        let mut score = Some(Decimal::ONE);
        let mut deep = true;
        for &(i, token_in) in &self.hops {
            let r = self.rates.get(self.ctx, self.g, i, token_in);
            score = score.zip(r).and_then(|(s, r)| s.checked_mul(r));
            deep &= self.rates.side(self.g, i, token_in).is_some_and(|s| s.1);
        }
        let Some(score) = score else {
            self.out.unrated += 1;
            return;
        };
        let up = score > Decimal::ONE;
        self.out.above_one += u64::from(up);
        if self.out.best.is_none_or(|b| score > b) {
            self.out.best = Some(score);
        }
        if deep {
            self.out.deep += 1;
            self.out.deep_above_one += u64::from(up);
            if self.out.best_deep.is_none_or(|b| score > b) {
                self.out.best_deep = Some(score);
            }
        }
        if self.floor.is_some_and(|f| (deep, score) < f) {
            return;
        }
        self.kept.push(Cand {
            deep,
            score,
            hops: self.hops.clone(),
            depth: None,
            key: Vec::new(),
        });
        if self.kept.len() >= self.top_n.saturating_mul(2).max(self.top_n + 1) {
            self.compact();
        }
    }

    /// Сжатие отбора до top_n по полному порядку (глубокий ↓, курс ↓, глубина узкого места ↓, ключ ↑); порог — N-й.
    /// Ошибок нет.
    fn compact(&mut self) {
        let mut kept = std::mem::take(&mut self.kept);
        for c in &mut kept {
            if c.key.is_empty() {
                c.depth = self.bottleneck(&c.hops);
                c.key = key(self.g, &c.hops);
            }
        }
        kept.sort_by(|a, b| {
            b.deep
                .cmp(&a.deep)
                .then(b.score.cmp(&a.score))
                .then(b.depth.cmp(&a.depth))
                .then(a.key.cmp(&b.key))
        });
        kept.truncate(self.top_n);
        if kept.len() == self.top_n {
            self.floor = kept.last().map(|c| (c.deep, c.score));
        }
        self.kept = kept;
    }

    /// Глубина узкого места цикла (USD): наименьшая глубина стороны пула; нет глубины у шага — None. Ошибок нет.
    fn bottleneck(&self, hops: &[(usize, Address)]) -> Option<Decimal> {
        let mut min: Option<Decimal> = None;
        for &(i, token_in) in hops {
            let (d, _) = self.rates.side(self.g, i, token_in)?;
            min = Some(min.map_or(d, |m| m.min(d)));
        }
        min
    }
}

/// Ключ цикла для порядка при равенстве: байты ссылки пула и токена входа каждого шага. Ошибок нет.
fn key(g: &Graph, hops: &[(usize, Address)]) -> Vec<u8> {
    let mut k = Vec::new();
    for &(i, token_in) in hops {
        match &g.edge(i).entry.pool.reference {
            PoolRef::Contract(a) => {
                k.push(0);
                k.extend_from_slice(a.as_slice());
            }
            PoolRef::Singleton { manager, id } => {
                k.push(1);
                k.extend_from_slice(manager.as_slice());
                k.extend_from_slice(id.as_slice());
            }
            PoolRef::Member { contract, coin } => {
                k.push(2);
                k.extend_from_slice(contract.as_slice());
                k.extend_from_slice(coin.as_slice());
            }
            PoolRef::Multi {
                contract,
                token0,
                token1,
            } => {
                k.push(3);
                k.extend_from_slice(contract.as_slice());
                k.extend_from_slice(token0.as_slice());
                k.extend_from_slice(token1.as_slice());
            }
        }
        k.extend_from_slice(token_in.as_slice());
    }
    k
}
