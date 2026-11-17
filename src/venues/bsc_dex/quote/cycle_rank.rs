//! Файл: venues/bsc_dex/quote/cycle_rank.rs
//! Что делает: решение 404 — отбор N ЛУЧШИХ циклов DEX ↔ DEX на базовую монету вместо «первых N найденных» (было:
//! quote/route_paths.rs enumerate_cycles обрывал перебор на dex_dex.max_cycles циклах в порядке рёбер графа — порядок
//! HashMap, то есть случайные несколько монет за проход). Перебор — тот же поиск в глубину по снимку графа (те же
//! правила: старт = финиш = базовая, 2..=dex_dex.max_cycle_len хопов, физический пул в цикле один раз, внутренние
//! токены не повторяются), но цикл не копируется: путь держится индексами рёбер, у каждого найденного цикла считается
//! дешёвый показатель — «курс круга» = произведение курсов рёбер с комиссией пула:
//! курс ребра = спот-цена пула по его текущему состоянию в снимке (quote/spot.rs coins_for — та же цена, что у сизинга
//! пробы, все виды пулов) × (1 − комиссия пула на этом направлении, quote/steps.rs fee_pct). Курс > 1 — по текущим ценам
//! круг возвращает больше, чем вложено, ещё до проскальзывания и газа; это не доказательство прибыли и не отсев
//! (строгий отсев — quote/bound_cycle.rs), только порядок: полный расчёт (ноги, вход, газ, форма) получают
//! dex_dex.max_cycles лучших.
//! Порядок полный и детерминированный: курс круга по убыванию; при равенстве — глубина узкого места (наименьшая глубина
//! стороны пула по книге глубины pools/depth_book.rs Book::last, USD) по убыванию; затем — ключ цикла (пулы и токен входа
//! каждого шага, байты) по возрастанию. Итог не зависит от порядка рёбер графа, пока предел dex_dex.rank_max_cycles не
//! достигнут. Отбор без копий: держится не больше 2·N кандидатов (сжатие до N по порогу N-го курса); цикл с курсом ниже
//! порога отбрасывается до выделения памяти. Память одного прохода (Rates, своя у прохода, без замков): курсы рёбер,
//! комиссия V2 по виду DEX (хранилище настроек — раз на вид за проход) и глубина стороны пула (книга глубины — раз на
//! ребро-сторону за проход, только у кандидатов при сжатии). Рёбра копируются только у N отобранных (как раньше у N первых).
//! Кто вызывает: quote/cycle_scan.rs (scan).
//! От чего зависит: pools/graph.rs (edge_ids, edge), pools/depth_book.rs (Book::last), quote/spot.rs, quote/steps.rs,
//! quote/route_paths.rs (RoutePath, Step), num.rs, ctx.rs.
//! Настройки: читает вызывающий — dex_dex.max_cycle_len, dex_dex.max_cycles, dex_dex.rank_max_cycles [bsc_dex];
//! через steps — dex.v2_fee_pct [вид пула V2].

use std::collections::HashMap;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{PoolKind, PoolRef};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::depth_book::Book;
use super::super::pools::graph::{Edge, Graph};
use super::route_paths::{RoutePath, Step};
use super::{spot, steps};

/// Масштаб точности деления цены (знаков): сумма встречного токена 10^27 минимальных единиц в coins_for — точность
/// арифметики (как 2^96 в spot.rs), а не параметр торговли: курс считается в отношении минимальных единиц.
const SCALE_DIGITS: u8 = 27;

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
    /// Наибольший курс круга среди просмотренных.
    pub best: Option<Decimal>,
    /// Циклов без курса (у ребра нет спот-цены или комиссии) — в отбор не идут.
    pub unrated: u64,
    /// Перебор остановлен пределом dex_dex.rank_max_cycles (отбор — из просмотренных).
    pub capped: bool,
}

/// Память одного прохода (своя у прохода, без замков; общая для всех базовых): курс ребра — по индексу ребра·2 +
/// (вход — монета пула); глубина стороны пула по книге — по тому же индексу (книга читается раз на ребро-сторону за
/// проход); комиссия V2 без fee_pips — по виду DEX (настройка dex.v2_fee_pct читается раз на вид за проход, а не на
/// ребро: ревью t189 — никаких чтений хранилища на каждом ребре горячего прохода).
pub struct Rates {
    rate: Vec<Option<Option<Decimal>>>,
    depth: HashMap<usize, Option<Decimal>>,
    v2_fee: HashMap<&'static str, Option<Decimal>>,
}

impl Rates {
    /// Пустая память на граф. Принимает: граф. Ошибок нет.
    pub fn new(g: &Graph) -> Rates {
        Rates {
            rate: vec![None; g.len().saturating_mul(2)],
            depth: HashMap::new(),
            v2_fee: HashMap::new(),
        }
    }

    /// Курс ребра i при входе token_in (из памяти или посчитан один раз). None — курса нет. Ошибок нет.
    fn get(&mut self, ctx: &Ctx, g: &Graph, i: usize, token_in: Address) -> Option<Decimal> {
        let e = g.edge(i);
        let slot = i * 2 + usize::from(token_in == e.a);
        if let Some(r) = self.rate[slot] {
            return r;
        }
        let r = self
            .fee(ctx, e, token_in)
            .and_then(|f| rate_with_fee(e, token_in, f));
        self.rate[slot] = Some(r);
        r
    }

    /// Комиссия пула, % — как quote/steps.rs fee_pct; у V2 без fee_pips — dex.v2_fee_pct вида DEX из памяти прохода
    /// (как quote/pool_quote.rs v2_fee_pct). None — комиссии нет. Ошибок нет.
    fn fee(&mut self, ctx: &Ctx, e: &Edge, token_in: Address) -> Option<Decimal> {
        if e.entry.pool.kind != PoolKind::V2 {
            return steps::fee_pct(ctx, &e.entry, token_in).ok();
        }
        if let Some(p) = e.entry.pool.fee_pips {
            return Some(Decimal::from(p) / Decimal::from(10_000u32));
        }
        let id = e.entry.family.id;
        *self
            .v2_fee
            .entry(id)
            .or_insert_with(|| ctx.dex_dec("dex.v2_fee_pct", id).ok())
    }

    /// Глубина стороны пула ребра i при входе token_in (USD, книга глубины; из памяти прохода). Ошибок нет.
    fn depth(&mut self, book: &Book, g: &Graph, i: usize, token_in: Address) -> Option<Decimal> {
        let e = g.edge(i);
        let slot = i * 2 + usize::from(token_in == e.a);
        *self.depth.entry(slot).or_insert_with(|| {
            book.last(&e.entry.pool.reference).map(|d| {
                // Вход — встречный токен: покупка монеты пула (buy); вход — монета: продажа (sell).
                if token_in == e.entry.usdt {
                    d.band.buy
                } else {
                    d.band.sell
                }
            })
        })
    }
}

/// Курс ребра: минимальных единиц выхода за единицу входа по спот-цене пула × (1 − комиссия %/100). Принимает: окружение
/// (комиссия V2), ребро, токен входа. Возвращает: курс; None — нет цены (пустой пул, нет binStep), нет комиссии,
/// переполнение. Ошибок нет. (Тот же расчёт, что у отбора; отбор берёт комиссию V2 из памяти прохода.)
pub fn edge_rate(ctx: &Ctx, e: &Edge, token_in: Address) -> Option<Decimal> {
    let fee = steps::fee_pct(ctx, &e.entry, token_in).ok()?;
    rate_with_fee(e, token_in, fee)
}

/// Курс ребра при известной комиссии пула (%). Ошибок нет: None — нет цены, переполнение, комиссия ≥ 100 %.
fn rate_with_fee(e: &Edge, token_in: Address, fee: Decimal) -> Option<Decimal> {
    let one = U256::from(10u8).pow(U256::from(SCALE_DIGITS));
    let coins = spot::coins_for(&e.entry, one)?;
    let per = num::to_decimal(coins, SCALE_DIGITS)?;
    if per <= Decimal::ZERO {
        return None;
    }
    let raw = if token_in == e.entry.coin {
        Decimal::ONE.checked_div(per)?
    } else {
        per
    };
    let keep = Decimal::ONE - fee / Decimal::ONE_HUNDRED;
    if keep <= Decimal::ZERO {
        return None;
    }
    raw.checked_mul(keep)
}

/// Кандидат отбора: курс, шаги (ребро, токен входа), глубина узкого места и ключ (заполняются при сжатии).
struct Cand {
    score: Decimal,
    hops: Vec<(usize, Address)>,
    depth: Option<Decimal>,
    key: Vec<u8>,
}

/// Состояние перебора одной базовой монеты.
struct Walk<'a> {
    ctx: &'a Ctx,
    g: &'a Graph,
    book: &'a Book,
    rates: &'a mut Rates,
    base: Address,
    max_len: usize,
    top_n: usize,
    cap: Option<u64>,
    visited: Vec<Address>,
    used: Vec<PoolRef>,
    hops: Vec<(usize, Address)>,
    kept: Vec<Cand>,
    floor: Option<Decimal>,
    out: Ranked,
}

/// Отбор top_n лучших циклов base → … → base. Принимает: окружение, снимок графа, книгу глубины, память курсов,
/// базовую, длину цикла (≥ 2), число лучших (> 0), предел просмотра (None — без предела). Возвращает: Ranked.
/// Ошибок нет: пределы проверяет вызывающий (cycle_scan::limits).
#[allow(clippy::too_many_arguments)]
pub fn top(
    ctx: &Ctx,
    g: &Graph,
    book: &Book,
    rates: &mut Rates,
    base: Address,
    max_len: usize,
    top_n: usize,
    cap: Option<u64>,
) -> Ranked {
    let out = Ranked {
        paths: Vec::new(),
        rates: Vec::new(),
        seen: 0,
        above_one: 0,
        best: None,
        unrated: 0,
        capped: false,
    };
    if max_len < 2 || top_n == 0 {
        return out;
    }
    let mut w = Walk {
        ctx,
        g,
        book,
        rates,
        base,
        max_len,
        top_n,
        cap,
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
    /// Обход в глубину (правила enumerate_cycles: пул по site один раз, токен один раз, замыкание в base). Ошибок нет.
    fn dfs(&mut self, node: Address) {
        let g = self.g;
        for &i in g.edge_ids(node) {
            if self.out.capped {
                return;
            }
            let e = g.edge(i);
            let site = e.entry.pool.reference.site();
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
            } else if self.hops.len() + 1 < self.max_len && !self.visited.contains(&next) {
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

    /// Замкнутый цикл в self.hops: учёт, курс, отбор. Ошибок нет.
    fn close(&mut self) {
        if self.cap.is_some_and(|c| self.out.seen >= c) {
            self.out.capped = true;
            return;
        }
        self.out.seen += 1;
        let mut score = Some(Decimal::ONE);
        for &(i, token_in) in &self.hops {
            let r = self.rates.get(self.ctx, self.g, i, token_in);
            score = score.zip(r).and_then(|(s, r)| s.checked_mul(r));
        }
        let Some(score) = score else {
            self.out.unrated += 1;
            return;
        };
        if score > Decimal::ONE {
            self.out.above_one += 1;
        }
        if self.out.best.is_none_or(|b| score > b) {
            self.out.best = Some(score);
        }
        if self.floor.is_some_and(|f| score < f) {
            return;
        }
        self.kept.push(Cand {
            score,
            hops: self.hops.clone(),
            depth: None,
            key: Vec::new(),
        });
        if self.kept.len() >= self.top_n.saturating_mul(2).max(self.top_n + 1) {
            self.compact();
        }
    }

    /// Сжатие отбора до top_n по полному порядку (курс ↓, глубина узкого места ↓, ключ ↑); порог — курс N-го. Ошибок нет.
    fn compact(&mut self) {
        let mut kept = std::mem::take(&mut self.kept);
        for c in &mut kept {
            if c.key.is_empty() {
                c.depth = self.bottleneck(&c.hops);
                c.key = key(self.g, &c.hops);
            }
        }
        kept.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then(b.depth.cmp(&a.depth))
                .then(a.key.cmp(&b.key))
        });
        kept.truncate(self.top_n);
        if kept.len() == self.top_n {
            self.floor = kept.last().map(|c| c.score);
        }
        self.kept = kept;
    }

    /// Глубина узкого места цикла (USD): наименьшая глубина стороны пула по книге; нет глубины у шага — None. Ошибок нет.
    fn bottleneck(&mut self, hops: &[(usize, Address)]) -> Option<Decimal> {
        let mut min: Option<Decimal> = None;
        let g = self.g;
        for &(i, token_in) in hops {
            let d = self.rates.depth(self.book, g, i, token_in)?;
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
