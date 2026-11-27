//! Файл: venues/bsc_dex/quote/route_best.rs
//! Что делает: решение 187 (Тигран, 06.11.2026: «не первые 20, а 20 лучших; чтобы мы чётко понимали, что это наши
//! правильные пути») — отбор путей графа по качеству, а не по порядку обхода. Раньше перебор (quote/route_paths.rs)
//! останавливался на первых dex.route_max_paths найденных путях: какие это будут — решал порядок рёбер в памяти
//! (случайный между сборками графа), и при многих промежуточных монетах прямой путь мог не попасть в выборку.
//! Теперь, если задана dex.route_search_max_paths:
//! 1. Перебор — короткие пути первыми: все пути из 1 хопа, потом из 2, …, до dex.route_max_hops; не больше
//!    dex.route_search_max_paths путей всего (предел скорости). Прямые пути и короткие обходы в выборку попадают всегда.
//! 2. Каждый путь считается целиком нашей математикой (quote/route_calc.rs: математика каждого пула по его реальным
//!    резервам/тикам, комиссия каждого пула, налог монеты на шаге, крючок) — дважды: на первую долю сплита
//!    (smart.split_step_pct от объёма — лучшая цена «с верха») и на весь объём (сможет ли путь один взять сделку и
//!    по какой цене). При dex.route_gas_aware = да — по ЧИСТОМУ результату: минус газ всех пулов пути в USDT
//!    (quote/route_gas.rs). Путь, не взявший даже первую долю, отбрасывается (сплит его и так не выбрал бы).
//! 3. В сплит идут dex.route_max_paths лучших: по очереди лучший «по первой доле», лучший «по всему объёму», второй
//!    «по первой доле» … без повторов. Так в выборке и самые дешёвые с верха пути, и самые глубокие.
//!
//! Дальше — прежний сплит (quote/graph_split.rs) и прежняя сборка котировки: деньги и исполнение не меняются.
//! Не задана dex.route_search_max_paths — ТОЧНО как раньше (первые dex.route_max_paths путей обхода, без отбора).
//! Решение 295: один многомонетный пул (PoolRef::site) — не больше одного шага пути (как route_paths.rs).
//! Решение 312: число расчётов пути (eval) в отборе — в замер котировки (quote/quote_timing.rs count), один раз за отбор.
//! Решение 336: расчёт путей в отборе — на dex.quote_threads потоках (quote/par.rs), итоги по порядку путей — отбор тот
//! же; брошенный расчёт останавливает и помощников.
//! Кто вызывает: quote/graph_route.rs.
//! От чего зависит: quote/par.rs, quote/abandon.rs, pools/graph.rs, quote/route_paths.rs (RoutePath), quote/route_hops.rs (решение 410: стек перебора по ссылкам), fast_hash.rs, quote/graph_split.rs (Cand, score, better),
//! quote/route_calc.rs (eval), quote/pool_tax.rs (coin_keep).
//! Настройки: dex.route_search_max_paths, dex.quote_threads (решение 336) [bsc_dex] (необязательные); через вызывающего — dex.route_max_hops,
//! dex.route_max_paths, smart.split_step_pct, dex.route_gas_aware.

use std::cmp::Ordering;
use std::collections::HashSet;

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{PoolRef, Side, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::fast_hash::FastSet;
use super::super::pools::graph::Graph;
use super::graph_split::{Cand, better, score};
use super::pool_tax;
use super::quote_timing::{self, Count};
use super::route_calc;
use super::route_hops::{self, Hop};
use super::route_paths::RoutePath;

/// Предел перебора путей (dex.route_search_max_paths). None — не задана: поведение как раньше.
/// Ошибки: NotConfigured — задана, но не больше нуля.
pub fn search_cap(ctx: &Ctx) -> GatewayResult<Option<usize>> {
    let Ok(v) = ctx.int("dex.route_search_max_paths") else {
        return Ok(None);
    };
    match usize::try_from(v) {
        Ok(n) if n > 0 => Ok(Some(n)),
        _ => Err(ctx.not_set(
            "dex.route_search_max_paths",
            "dex.route_search_max_paths должна быть больше нуля",
        )),
    }
}

/// Перебор путей `from` → `to`, короткие первыми: сначала все пути длиной 1, потом 2, … до `max_hops`; всего не
/// больше `cap`. Правила пути — как в route_paths::enumerate: внутренние узлы только из `allowed`, без повторного
/// захода в токен. Возвращает: пути (может быть пусто). Ошибок нет.
pub fn enumerate(
    graph: &Graph,
    from: Address,
    to: Address,
    allowed: &HashSet<Address>,
    max_hops: usize,
    cap: usize,
) -> Vec<RoutePath> {
    let mut out = Vec::new();
    if from == to {
        return out;
    }
    for len in 1..=max_hops {
        if out.len() >= cap {
            break;
        }
        // Решение 410 (t195): хеш foldhash (fast_hash.rs), только «есть ли».
        let mut visited: FastSet<Address> = FastSet::default();
        visited.insert(from);
        let mut sites: FastSet<PoolRef> = FastSet::default();
        // Решение 410 (t195): стек — ссылки на рёбра (route_hops.rs).
        let mut steps: Vec<Hop<'_>> = Vec::new();
        let w = Walk {
            graph,
            to,
            allowed,
            len,
            cap,
        };
        w.dfs(from, &mut visited, &mut sites, &mut steps, &mut out);
    }
    out
}

/// Обход ровно на `len` рёбер.
struct Walk<'a> {
    graph: &'a Graph,
    to: Address,
    allowed: &'a HashSet<Address>,
    len: usize,
    cap: usize,
}

impl<'a> Walk<'a> {
    /// Из `node` (пройдено steps.len() рёбер): последнее ребро — только в цель, внутренние — в разрешённую
    /// промежуточную, где ещё не были. Решение 295: другая пара уже пройденного многомонетного пула (`sites`) в путь не
    /// идёт — как в route_paths::enumerate. Ошибок нет.
    fn dfs(
        &self,
        node: Address,
        visited: &mut FastSet<Address>,
        sites: &mut FastSet<PoolRef>,
        steps: &mut Vec<Hop<'a>>,
        out: &mut Vec<RoutePath>,
    ) {
        for edge in self.graph.edges_from(node) {
            if out.len() >= self.cap {
                return;
            }
            let multi = [edge.entry.shared_market(), edge.entry.second_liquidity_site()];
            if multi.iter().flatten().any(|s| sites.contains(s)) || route_hops::physical_overlap(edge, steps, sites) {
                continue;
            }
            let next = edge.other(node);
            // Решение 410 (t195): шаг — ссылка на ребро; раньше копия ребра (с записью пула) делалась на КАЖДОЕ ребро
            // узла, хотя на последнем шаге в путь идут только рёбра в цель. Копия — только в выданный путь.
            let step: Hop<'a> = (edge, node, next);
            let last = steps.len() + 1 == self.len;
            if last {
                if next == self.to {
                    out.push(route_hops::path(steps, step));
                }
            } else if next != self.to && self.allowed.contains(&next) && !visited.contains(&next) {
                visited.insert(next);
                for s in multi.iter().flatten() {
                    sites.insert(s.clone());
                }
                steps.push(step);
                self.dfs(next, visited, sites, steps, out);
                steps.pop();
                for s in multi.iter().flatten() {
                    sites.remove(s);
                }
                visited.remove(&next);
            }
        }
    }
}

/// Порядок по качеству для стороны: покупка — меньше USDT лучше, продажа — больше. Ошибок нет.
fn order(side: Side, a: Decimal, b: Decimal) -> Ordering {
    if better(side, a, b) {
        Ordering::Less
    } else if better(side, b, a) {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

/// Отбор `keep` лучших путей (см. шапку, п. 2–3). Принимает: окружение, кандидатов (ноги и газ пути в USDT),
/// сторону, количество монет, налоги монеты (запасные, как у сплита), сколько оставить.
/// Возвращает: кандидатов в порядке качества. Ошибки: NotConfigured — smart.split_step_pct не задан / вне (0; 100].
pub fn best(
    ctx: &Ctx,
    cands: Vec<Cand>,
    side: Side,
    qty: Decimal,
    t: &TokenTaxes,
    keep: usize,
) -> GatewayResult<Vec<Cand>> {
    let step_pct = ctx.dec("smart.split_step_pct")?;
    if step_pct <= Decimal::ZERO || step_pct > Decimal::ONE_HUNDRED {
        return Err(ctx.not_set(
            "smart.split_step_pct",
            "smart.split_step_pct должен быть в (0; 100]",
        ));
    }
    let unit = qty * step_pct / Decimal::ONE_HUNDRED;
    // Чистый результат пути на объём: USDT пути ± газ пути (газ = 0, если учёт газа выключен).
    let net = |c: &Cand, q: Decimal| -> Option<Decimal> {
        let tax_keep = pool_tax::coin_keep(&c.legs, side, t);
        let p = route_calc::eval(ctx, &c.legs, side, q, tax_keep).ok()?;
        Some(score(side, p.usdt, true, c.gas_usdt))
    };
    // Путь: на первую долю, и только если она взята, — на весь объём (как раньше).
    let both = |c: &Cand| -> (Option<Decimal>, Option<Decimal>) {
        match net(c, unit) {
            None => (None, None),
            Some(u) => (Some(u), net(c, qty)),
        }
    };
    // Решение 336: пути считаются независимо — на dex.quote_threads потоках (quote/par.rs; итоги по порядку путей,
    // выбор тот же). Брошенный расчёт (quote/abandon.rs) — отказ Timeout, как у проверки перед отбором.
    let want = super::par::want(ctx, "dex.quote_threads");
    let alive = || !super::abandon::is_set();
    let res = super::par::map(ctx, want, &cands, both, &alive).ok_or_else(|| {
        ctx.err(
            ErrorCode::Timeout,
            "расчёт брошен (срок расчёта вышел) — отбор путей графа не выполняется",
        )
    })?;
    let mut top: Vec<(usize, Decimal)> = Vec::new();
    let mut deep: Vec<(usize, Decimal)> = Vec::new();
    // Решение 312: счёт расчётов — как раньше: первая доля у каждого пути, весь объём — у взявших первую долю.
    let mut evals = 0u64;
    for (i, (u, f)) in res.into_iter().enumerate() {
        evals += 1;
        let Some(u) = u else { continue };
        evals += 1;
        top.push((i, u));
        if let Some(f) = f {
            deep.push((i, f));
        }
    }
    // Устойчивая сортировка: при равенстве — порядок перебора (короткие первыми).
    top.sort_by(|a, b| order(side, a.1, b.1));
    deep.sort_by(|a, b| order(side, a.1, b.1));
    quote_timing::count(Count::BestEvals, evals);
    let picked = interleave(&top, &deep, keep);
    let mut slots: Vec<Option<Cand>> = cands.into_iter().map(Some).collect();
    Ok(picked
        .into_iter()
        .filter_map(|i| slots.get_mut(i).and_then(Option::take))
        .collect())
}

/// По очереди из двух рейтингов, без повторов, не больше `keep`. Возвращает индексы. Ошибок нет.
pub fn interleave(a: &[(usize, Decimal)], b: &[(usize, Decimal)], keep: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let (mut ia, mut ib) = (0, 0);
    while out.len() < keep && (ia < a.len() || ib < b.len()) {
        for (list, at) in [(a, &mut ia), (b, &mut ib)] {
            // Пропуск уже взятых — до первого нового в этом рейтинге.
            while *at < list.len() && seen.contains(&list[*at].0) {
                *at += 1;
            }
            if out.len() < keep && *at < list.len() {
                seen.insert(list[*at].0);
                out.push(list[*at].0);
                *at += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleave_takes_turns_without_repeats() {
        let d = Decimal::from;
        let a = [(2, d(1)), (0, d(2)), (5, d(3))];
        let b = [(0, d(1)), (2, d(2)), (7, d(3))];
        assert_eq!(interleave(&a, &b, 10), vec![2, 0, 5, 7]);
        assert_eq!(interleave(&a, &b, 3), vec![2, 0, 5]);
        assert_eq!(interleave(&a, &[], 2), vec![2, 0]);
    }

    #[test]
    fn order_by_side() {
        let (lo, hi) = (Decimal::ONE, Decimal::TWO);
        assert_eq!(order(Side::Buy, lo, hi), Ordering::Less);
        assert_eq!(order(Side::Sell, lo, hi), Ordering::Greater);
    }
}
