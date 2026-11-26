//! Файл: venues/bsc_dex/quote/graph_split.rs
//! Что делает: жадный сплит объёма между путями графа (решения 38, 80; вынесен из quote/graph_route.rs). Объём режется
//! на шаги smart.split_step_pct, каждый шаг отдаётся пути с лучшей предельной отдачей (покупка — меньше USDT за шаг,
//! продажа — больше), не больше smart.max_splits путей.
//! Решение 122, часть R1 (учёт газа, как `gasInclude` у KyberSwap): при dex.route_gas_aware = да отдача шага — ЧИСТАЯ:
//! открытие нового пути стоит его газа в USDT (quote/route_gas.rs), увеличение уже открытой доли газа не добавляет
//! (газ обмена от суммы почти не зависит — он измерен пробой на пул). Покупка: цена шага = USDT шага + газ нового пути;
//! продажа: отдача шага = USDT шага − газ нового пути. Поэтому выбор между путями разной длины — тоже по чистому
//! выходу, а новая доля добавляется, только если её чистая отдача лучше увеличения уже открытой. Продажа: новая доля
//! с чистой отдачей ≤ 0 (газ съедает прирост) не открывается, пока шаг принимает открытая доля. Объём сделки
//! фиксирован (Тигран, 03.11): остаток не теряется — идёт в открытые доли, а если они его уже не принимают, открывается
//! лучший новый путь. Не задано / «нет» — сплит без газа, как до решения 122.
//! Решение 223 (живой случай FLUX 08.11): доли, делящие пул, — НЕ независимая ликвидность. Исполнитель и бандл проводят
//! доли по порядку маршрута (порядок кандидатов), каждая — своим обменом в общем пуле, поэтому шаг пути, у которого есть
//! общий пул с уже открытой долей, оценивается совместно: все открытые доли и этот путь (с шагом) — по порядку на
//! последовательных состояниях общих пулов (quote/graph_seq.rs), отдача шага = прирост СУММЫ выходов (чужие доли после
//! него в общем пуле теряют — это входит в отдачу). Путь без общих пулов с открытыми долями считается как раньше
//! (побайтно). Общий пул вида без расчёта состояния после обмена (quote/pool_after.rs) — вторая доля через него не
//! открывается. Итоговые доли с общими пулами — пересчёт по порядку на последовательных состояниях.
//! Решение 295: «общий пул» двух долей — по физическому пулу (PoolRef::site).
//! Решение 312: шаги и расчёты сплита — в замер котировки (quote/quote_timing.rs count), один раз за сплит.
//! Решение 336: оценка шага каждого пути — на dex.quote_threads потоках (quote/par.rs, try_step — только чтение долей);
//! выбор шага — по порядку кандидатов, как раньше: доли те же. Брошенный расчёт останавливает и помощников.
//! Кто вызывает: quote/graph_route.rs.
//! От чего зависит: quote/par.rs, quote/abandon.rs, quote/route_calc.rs (Leg, eval), quote/pool_tax.rs (налог пула шага монеты, решение 146), quote/path.rs (Path),
//! quote/graph_seq.rs (общие пулы, решение 223).
//! Настройки: smart.split_step_pct, smart.max_splits, dex.quote_threads (решение 336) [bsc_dex]; учёт газа включает вызывающий (dex.route_gas_aware).

use rust_decimal::Decimal;

use crate::core::types::{Side, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::direct::ceil_units;
use super::path::Path;
use super::quote_timing::{self, Count};
use super::route_calc;
use super::{graph_seq, pool_tax};

/// Путь-кандидат сплита: ноги в порядке исполнения, сколько монет ему уже отдано, его расчёт и газ пути в USDT
/// (0 — учёт газа выключен).
pub struct Cand {
    pub legs: Vec<route_calc::Leg>,
    pub qty: Decimal,
    pub got: Option<Path>,
    pub gas_usdt: Decimal,
}

impl Cand {
    /// Кандидат без отданного объёма. Ошибок нет.
    pub fn new(legs: Vec<route_calc::Leg>, gas_usdt: Decimal) -> Self {
        Self {
            legs,
            qty: Decimal::ZERO,
            got: None,
            gas_usdt,
        }
    }
}

/// Шаг-кандидат: путь, новые пути затронутых долей (индекс кандидата, путь), чистая отдача шага.
type Pick = (usize, Vec<(usize, Path)>, Decimal);

/// Чистая отдача шага: прирост USDT шага (покупка — отдадим больше, продажа — получим больше) с газом нового пути.
/// Покупка — меньше лучше, продажа — больше лучше. Ошибок нет.
pub fn score(side: Side, delta: Decimal, opens: bool, gas: Decimal) -> Decimal {
    let g = if opens { gas } else { Decimal::ZERO };
    match side {
        Side::Buy => delta + g,
        Side::Sell => delta - g,
    }
}

/// s лучше d для стороны (покупка — меньше, продажа — больше). Ошибок нет.
pub fn better(side: Side, s: Decimal, d: Decimal) -> bool {
    match side {
        Side::Buy => s < d,
        Side::Sell => s > d,
    }
}

/// Выбор шага между увеличением открытой доли и открытием нового пути. Новый путь берётся, только если его чистая
/// отдача лучше; у продажи при учёте газа новый путь с чистой отдачей ≤ 0 (газ съедает прирост) не открывается, пока
/// шаг может принять открытая доля — даже если её прирост мал. Объём сделки фиксирован: если открытые доли шаг не
/// принимают (ликвидность в прочитанных пределах кончилась), открывается лучший новый путь, иначе остаток не
/// исполнился бы. Ошибок нет.
fn choose<T>(
    side: Side,
    gas_aware: bool,
    open: Option<(usize, T, Decimal)>,
    new: Option<(usize, T, Decimal)>,
) -> Option<(usize, T, Decimal)> {
    match (open, new) {
        (Some(o), Some(n)) => {
            let unpaid = gas_aware && side == Side::Sell && n.2 <= Decimal::ZERO;
            // Равенство — как прежний общий перебор: раньше по порядку кандидатов (меньший индекс).
            let wins = better(side, n.2, o.2) || (n.2 == o.2 && n.0 < o.0);
            if !unpaid && wins { Some(n) } else { Some(o) }
        }
        (o, n) => o.or(n),
    }
}

/// Сплит. Принимает: окружение, кандидатов, сторону, количество монет, налоги монеты из проверки (запасные: у каждого
/// пути налог — пула его шага монеты, quote/pool_tax.rs coin_keep, решение 146), учитывать ли газ.
/// Возвращает: доли (пути). Ошибки: NotConfigured (шаг/число долей), NoRoute (ни один путь не принял объём).
pub fn split(
    ctx: &Ctx,
    cands: &mut [Cand],
    side: Side,
    qty: Decimal,
    t: &TokenTaxes,
    gas_aware: bool,
) -> GatewayResult<Vec<Path>> {
    let max_splits = usize::try_from(ctx.int("smart.max_splits")?).unwrap_or(usize::MAX);
    let step_pct = ctx.dec("smart.split_step_pct")?;
    if step_pct <= Decimal::ZERO || step_pct > Decimal::ONE_HUNDRED || max_splits == 0 {
        return Err(ctx.not_set(
            "smart.split_step_pct",
            "smart.split_step_pct должен быть в (0; 100], smart.max_splits — больше нуля",
        ));
    }
    let unit = qty * step_pct / Decimal::ONE_HUNDRED;
    // Решение 336: потоков на шаг сплита (не задана — один, как раньше).
    let want = super::par::want(ctx, "dex.quote_threads");
    let mut left = qty;
    // Решение 312: шаги и расчёты — локально, в замер котировки один раз после цикла.
    let (mut steps, mut evals) = (0u64, 0u64);
    while left > Decimal::ZERO {
        steps += 1;
        let step = unit.min(left);
        let used = cands.iter().filter(|c| c.got.is_some()).count();
        // Лучшее увеличение уже открытой доли и лучшее открытие нового пути — отдельно (R1: газ — только у нового).
        // Выбранный шаг несёт новые пути всех затронутых долей (решение 223: общий пул меняет и чужие доли).
        let mut open: Option<Pick> = None;
        let mut new: Option<Pick> = None;
        // Решение 336: шаг каждого пути считается независимо (только чтение долей) — на dex.quote_threads потоках
        // (quote/par.rs); выбор ниже — по порядку кандидатов, как раньше, поэтому доли те же.
        let idx: Vec<usize> = (0..cands.len())
            .filter(|&i| !(cands[i].got.is_none() && used >= max_splits))
            .collect();
        evals += idx.len() as u64;
        let view: &[Cand] = &*cands;
        let one = |&i: &usize| try_step(ctx, view, i, step, side, t);
        let alive = || !super::abandon::is_set();
        let res = super::par::map(ctx, want, &idx, one, &alive).ok_or_else(|| {
            ctx.err(
                ErrorCode::Timeout,
                "расчёт брошен (срок расчёта вышел) — сплит графа не выполняется",
            )
        })?;
        for (&i, r) in idx.iter().zip(res) {
            let Some((paths, delta)) = r else { continue };
            let c = &cands[i];
            let opens = c.got.is_none();
            let s = score(side, delta, opens, c.gas_usdt);
            let slot = if opens { &mut new } else { &mut open };
            if slot.as_ref().is_none_or(|(_, _, d)| better(side, s, *d)) {
                *slot = Some((i, paths, s));
            }
        }
        let pick = choose(side, gas_aware, open, new);
        // Ни один путь шаг не принял — остаток не отдан: доли меньше qty. Это не теряется молча: costs::quote ставит
        // fillable_qty = сумма монет долей (< qty), ядро такую котировку отвергает (core/costs_quote.rs, trade_ops.rs).
        let Some((i, mut paths, _)) = pick else { break };
        let mut assigned = cands[i].qty + step;
        // Два накопителя Decimal (left и доля) иногда расходятся на последнем шаге. Если весь
        // объём ушёл одному пути продажи, проверяем его исходный запрос на фактических атомах.
        if side == Side::Sell
            && step == left
            && used == 1
            && cands[i].got.is_some()
            && assigned < qty
            && let [(j, path)] = paths.as_mut_slice()
            && *j == i
            && let Some(exact) = exact_single_sell(ctx, &cands[i], assigned, qty, t)
        {
            *path = exact;
            assigned = qty;
        }
        cands[i].qty = assigned;
        for (j, p) in paths {
            cands[j].got = Some(p);
        }
        left -= step;
    }
    quote_timing::count(Count::SplitSteps, steps);
    quote_timing::count(Count::SplitEvals, evals);
    // Решение 223: доли с общими пулами — итог по порядку на последовательных состояниях (тот же расчёт, что в шагах;
    // явно, чтобы итог был ровно моделью исполнения). Без общих пулов — как раньше.
    let open: Vec<(&[route_calc::Leg], Decimal)> = cands
        .iter()
        .filter(|c| c.got.is_some())
        .map(|c| (c.legs.as_slice(), c.qty))
        .collect();
    let refs: Vec<&[route_calc::Leg]> = open.iter().map(|(l, _)| *l).collect();
    let legs: Vec<Path> = if graph_seq::shared(&refs).is_empty() {
        cands.iter_mut().filter_map(|c| c.got.take()).collect()
    } else {
        graph_seq::eval(ctx, &open, side, t)?
    };
    if legs.is_empty() {
        return Err(ctx.err(ErrorCode::NoRoute, "граф: ни один путь не принял объём"));
    }
    Ok(legs)
}

/// Балл шага остаётся верным только при том же фактическом входе после налога. Дробный
/// запрос глубины должен помещаться и при округлении вверх до атома, а денежная котировка
/// строится по исходному количеству. При отказе сохраняется уже проверенная частичная доля.
fn exact_single_sell(
    ctx: &Ctx,
    c: &Cand,
    assigned: Decimal,
    qty: Decimal,
    t: &TokenTaxes,
) -> Option<Path> {
    let decimals = c.legs.first()?.dec_in;
    let keep = pool_tax::coin_keep(&c.legs, Side::Sell, t);
    if num::to_units(assigned * keep, decimals)? != num::to_units(qty * keep, decimals)? {
        return None;
    }
    let upper = num::to_decimal(ceil_units(qty, decimals)?, decimals)?;
    let sell = |q| route_calc::eval(ctx, &c.legs, Side::Sell, q, keep).ok();
    match upper.cmp(&qty) {
        std::cmp::Ordering::Less => return None,
        std::cmp::Ordering::Greater => {
            sell(upper)?;
        }
        std::cmp::Ordering::Equal => {}
    }
    sell(qty)
}

/// Шаг пути `i`: новые пути затронутых долей и прирост их USDT (решение 223: общий пул с открытой долей — совместно,
/// иначе — путь один). Только чтение кандидатов — можно считать параллельно (решение 336). None — путь шаг не принял.
/// Ошибок нет.
fn try_step(
    ctx: &Ctx,
    cands: &[Cand],
    i: usize,
    step: Decimal,
    side: Side,
    t: &TokenTaxes,
) -> Option<(Vec<(usize, Path)>, Decimal)> {
    if partners(cands, i) {
        return joint(ctx, cands, i, step, side, t);
    }
    let c = &cands[i];
    let tax_keep = pool_tax::coin_keep(&c.legs, side, t);
    let p = route_calc::eval(ctx, &c.legs, side, c.qty + step, tax_keep).ok()?;
    let before = c.got.as_ref().map_or(Decimal::ZERO, |g| g.usdt);
    let d = p.usdt - before;
    Some((vec![(i, p)], d))
}

/// Есть ли у пути `i` общий пул с другой открытой долей (решение 223). Ошибок нет.
fn partners(cands: &[Cand], i: usize) -> bool {
    let mine = &cands[i].legs;
    cands.iter().enumerate().any(|(j, c)| {
        j != i
            && c.got.is_some()
            && c.legs.iter().any(|a| {
                // Решение 295: по физическому пулу (site) — пары одного многомонетного пула тоже «общий пул».
                mine.iter()
                    .any(|b| a.entry.liquidity_site() == b.entry.liquidity_site())
            })
    })
}

/// Совместная оценка шага пути `i` (решение 223): открытые доли и путь `i` (его количество + шаг) — по порядку
/// кандидатов (= порядок исполнения долей) на последовательных состояниях общих пулов (quote/graph_seq.rs).
/// Возвращает: новые пути этих долей и прирост суммы их USDT против текущих путей открытых долей; None — не считается
/// (общий пул вида без расчёта состояния после обмена — вторая доля через него не открывается; доля не исполнима на
/// сдвинутом пуле). Ошибок иных нет.
fn joint(
    ctx: &Ctx,
    cands: &[Cand],
    i: usize,
    step: Decimal,
    side: Side,
    t: &TokenTaxes,
) -> Option<(Vec<(usize, Path)>, Decimal)> {
    let idx: Vec<usize> = (0..cands.len())
        .filter(|&j| j == i || cands[j].got.is_some())
        .collect();
    let shares: Vec<(&[route_calc::Leg], Decimal)> = idx
        .iter()
        .map(|&j| {
            let add = if j == i { step } else { Decimal::ZERO };
            (cands[j].legs.as_slice(), cands[j].qty + add)
        })
        .collect();
    let paths = graph_seq::eval(ctx, &shares, side, t).ok()?;
    let now: Decimal = cands
        .iter()
        .filter_map(|c| c.got.as_ref())
        .map(|g| g.usdt)
        .sum();
    let next: Decimal = paths.iter().map(|p| p.usdt).sum();
    Some((idx.into_iter().zip(paths).collect(), next - now))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Газ считается только при открытии пути и в нужную сторону. Ошибок нет.
    #[test]
    fn unpaid_new_share_goes_to_open_unless_open_is_full() {
        let (z, one) = (Decimal::ZERO, Decimal::ONE);
        let open = || Some((0usize, (), Decimal::from(5)));
        let new = || Some((1usize, (), -one));
        // Продажа с учётом газа: новый путь не окупает газ — шаг идёт в открытую долю.
        assert_eq!(
            choose(Side::Sell, true, open(), new()).map(|x| x.0),
            Some(0)
        );
        // Открытая доля шаг не принимает — открывается новый путь (объём не теряется).
        assert_eq!(choose(Side::Sell, true, None, new()).map(|x| x.0), Some(1));
        // Новый путь лучше открытой — берётся.
        let good = Some((1usize, (), Decimal::from(7)));
        assert_eq!(choose(Side::Sell, true, open(), good).map(|x| x.0), Some(1));
        // Покупка: меньше лучше.
        let cheap = Some((1usize, (), z));
        assert_eq!(choose(Side::Buy, true, open(), cheap).map(|x| x.0), Some(1));
    }

    #[test]
    fn score_charges_gas_on_open_only() {
        let (d, g) = (Decimal::from(10), Decimal::from(3));
        assert_eq!(score(Side::Buy, d, true, g), Decimal::from(13));
        assert_eq!(score(Side::Buy, d, false, g), d);
        assert_eq!(score(Side::Sell, d, true, g), Decimal::from(7));
        assert_eq!(score(Side::Sell, d, false, g), d);
    }
}
