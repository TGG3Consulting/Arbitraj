//! Файл: venues/bsc_dex/pools/graph_work.rs
//! Что делает: решение 308 (задача t137) — НАКОПИТЕЛЬНАЯ карта графа маршрутов у фонового владельца (graph_owner.rs):
//! оценка каждого пула (graph_eval.rs Rec), кандидаты по парам, топ-K пар (dex.route_max_pools_per_edge самых глубоких),
//! зависимости по курсу (встречный токен X ≠ USDT → пулы с этим встречным). Изменился пул — меняется только его запись,
//! его пара помечается к пересборке топа; изменился пул «X — USDT» — expand добавляет к пересчёту все пулы со встречным X
//! (их глубина в USD зависит от курса X; кривая — из памяти, решение 307). publish собирает новый Graph: топ
//! пересчитывается только у помеченных пар, остальное — клоны Arc готовых рёбер (O(E), без обхода N пулов реестра).
//! Тот же тип — у сборки с нуля (graph_full.rs): карта, где каждый пул оценён по снимку реестра.
//! Порядок топа: ранг по убыванию, ничья — по ссылке пула (depth_book::label) — детерминированно (у build ничья — в порядке
//! обхода HashMap; в новом режиме сравнение карт множествами требует детерминизма).
//! picture — картина «пара → [(пул, ранг)]» для сверки (graph_diff.rs).
//! Кто вызывает: graph_round.rs, graph_full.rs, graph_owner.rs.
//! От чего зависит: graph.rs (Graph::from_edges, Edge), graph_eval.rs (Rec), graph_snap.rs (Basis), depth_book.rs (label),
//! registry.rs.
//! Настройки: dex.route_max_pools_per_edge — значение даёт вызывающий.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::PoolRef;

use super::depth_book::label;
use super::graph::{Edge, Graph};
use super::graph_eval::Rec;
use super::graph_snap::Basis;
use super::registry::Registry;

/// Пара токенов (концы по возрастанию адреса).
type Pair = (Address, Address);

/// Картина для сверки: пара → отсортированные (пул текстом, ранг).
pub type Picture = BTreeMap<Pair, Vec<(String, Decimal)>>;

/// Накопительная карта.
pub struct Work {
    pub basis: Basis,
    recs: HashMap<PoolRef, Rec>,
    /// Кандидаты рёбер по парам.
    pairs: HashMap<Pair, HashSet<PoolRef>>,
    /// Встречный токен X (≠ USDT) → все пулы с этим встречным (зависят от курса X).
    deps: HashMap<Address, HashSet<PoolRef>>,
    /// Готовый топ пары.
    top: HashMap<Pair, Vec<Arc<Edge>>>,
    /// Пары, у которых топ надо пересобрать.
    dirty: HashSet<Pair>,
}

impl Work {
    /// Пустая карта на basis. Ошибок нет.
    pub fn new(basis: Basis) -> Work {
        Work {
            basis,
            recs: HashMap::new(),
            pairs: HashMap::new(),
            deps: HashMap::new(),
            top: HashMap::new(),
            dirty: HashSet::new(),
        }
    }

    /// Сколько пулов в карте. Ошибок нет.
    pub fn len(&self) -> usize {
        self.recs.len()
    }

    /// Пуста ли карта. Ошибок нет.
    pub fn is_empty(&self) -> bool {
        self.recs.is_empty()
    }

    /// Новая оценка пула `r` (None — пула больше нет). Ошибок нет.
    pub fn apply(&mut self, r: PoolRef, rec: Option<Rec>) {
        if let Some(old) = self.recs.remove(&r) {
            if old.cand.is_some()
                && let Some(s) = self.pairs.get_mut(&old.pair)
            {
                s.remove(&r);
            }
            self.dirty.insert(old.pair);
            if old.counter != self.basis.usdt
                && let Some(s) = self.deps.get_mut(&old.counter)
            {
                s.remove(&r);
            }
        }
        let Some(rec) = rec else {
            return;
        };
        if rec.cand.is_some() {
            self.pairs.entry(rec.pair).or_default().insert(r.clone());
        }
        self.dirty.insert(rec.pair);
        if rec.counter != self.basis.usdt {
            self.deps.entry(rec.counter).or_default().insert(r.clone());
        }
        self.recs.insert(r, rec);
    }

    /// Что пересчитать при изменении `refs`: сами пулы и — если пул «X — USDT» (встречный USDT) по прежней или текущей
    /// записи — все пулы со встречным X. Принимает: реестр, изменённые пулы, куда добавить. Ошибок нет.
    pub fn expand(&self, reg: &Registry, refs: &HashSet<PoolRef>, out: &mut HashSet<PoolRef>) {
        for r in refs {
            out.insert(r.clone());
            let old = self.recs.get(r).map(|x| (x.coin, x.counter));
            let now = reg.entry(r).map(|e| (e.coin, e.usdt));
            for (coin, counter) in [old, now].into_iter().flatten() {
                if counter == self.basis.usdt
                    && let Some(d) = self.deps.get(&coin)
                {
                    out.extend(d.iter().cloned());
                }
            }
        }
    }

    /// Пересобрать топ помеченных пар. Принимает: dex.route_max_pools_per_edge. Возвращает: сколько пар пересобрано.
    /// Ошибок нет.
    pub fn refresh(&mut self, per_edge: usize) -> usize {
        let dirty: Vec<Pair> = self.dirty.drain().collect();
        for pair in &dirty {
            let mut list: Vec<(Decimal, String, Arc<Edge>)> = self
                .pairs
                .get(pair)
                .into_iter()
                .flatten()
                .filter_map(|r| {
                    let (rank, edge) = self.recs.get(r)?.cand.clone()?;
                    Some((rank, label(r), edge))
                })
                .collect();
            list.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));
            list.truncate(per_edge);
            if list.is_empty() {
                self.top.remove(pair);
            } else {
                self.top
                    .insert(*pair, list.into_iter().map(|x| x.2).collect());
            }
        }
        dirty.len()
    }

    /// Граф из карты (топ помеченных пар пересобирается). Принимает: dex.route_max_pools_per_edge. Возвращает: граф и
    /// сколько пар пересобрано. Ошибок нет.
    pub fn publish(&mut self, per_edge: usize) -> (Graph, usize) {
        let pairs = self.refresh(per_edge);
        let edges: Vec<Arc<Edge>> = self.top.values().flatten().cloned().collect();
        (Graph::from_edges(edges), pairs)
    }

    /// Картина для сверки (после refresh/publish). Ошибок нет.
    pub fn picture(&self) -> Picture {
        self.top
            .iter()
            .map(|(pair, edges)| {
                let mut v: Vec<(String, Decimal)> = edges
                    .iter()
                    .map(|e| {
                        let r = &e.entry.pool.reference;
                        let rank = self
                            .recs
                            .get(r)
                            .and_then(|x| x.cand.as_ref())
                            .map(|c| c.0)
                            .unwrap_or_default();
                        (label(r), rank)
                    })
                    .collect();
                v.sort();
                (*pair, v)
            })
            .collect()
    }
}
