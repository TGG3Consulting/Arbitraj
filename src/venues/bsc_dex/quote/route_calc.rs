//! Файл: venues/bsc_dex/quote/route_calc.rs
//! Что делает: расчёт пути обмена из любого числа последовательных пулов (хопов) — обобщение двухшагового пути
//! через промежуточную монету (решение 52, quote/via.rs) до маршрута глубины 2–3 будущего графа (решение 80).
//! Математика и порядок операций — ровно те же, что в quote/via.rs, только для N хопов. Покупка решается «с конца»
//! (от нужного количества монеты назад к USDT, каждый пул отдаёт точный выход), продажа — «с начала» (от количества
//! монеты вперёд к USDT, каждый пул принимает точный вход).
//! Налог монеты применяется на стороне монеты (покупка — к выходу последнего хопа, продажа — ко входу первого; долю
//! keep передаёт вызывающий — налог ИМЕННО пула этого шага, quote/pool_tax.rs coin_keep, решение 146),
//! крючок каждого пула — к его выходу. Шаг, не исполнимый целиком в прочитанных пределах, — маршрута нет (NoRoute).
//! Решение 336: eval_from (route_first.rs first) — обмен хопа, который считается первым, можно посчитать один раз (via_pools.rs).
//! Решение «335 (лесенка влияния)»: доля крючка шага — по его входу (buy_step: settle; sell_step: pct_at) — один и тот же
//! расчёт у eval и route_first.rs first.
//! Кто вызывает: quote/via.rs (двухшаговый путь через промежуточную); позже — граф-маршрутизатор (решение 80).
//! От чего зависит: quote/path.rs (Hop, Path), quote/select.rs (Usable), quote/pool_quote.rs,
//! quote/direct.rs (ceil_units), pools/registry.rs (Entry), num.rs, math/swap_cl.rs (Swapped).
//! Настройки: нет своих; через pool_quote — dex.v2_fee_pct [вид пула V2].

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::Side;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::swap_cl::Swapped;
use super::super::num;
use super::super::pools::registry::Entry;
use super::direct::ceil_units;
use super::path::{Hop, Path, hop};
use super::pool_quote;
use super::select::Usable;

/// Один пул маршрута в порядке исполнения: пул, его допуск (крючок, газ) и токены/decimals входа и выхода.
#[derive(Clone)]
pub struct Leg {
    pub entry: Entry,
    pub usable: Usable,
    pub token_in: Address,
    pub token_out: Address,
    pub dec_in: u8,
    pub dec_out: u8,
}

/// Доля keep из процента: 1 − pct / 100. Ошибок нет.
pub fn keep(pct: Decimal) -> Decimal {
    Decimal::ONE - pct / Decimal::ONE_HUNDRED
}

/// Расчёт маршрута. Принимает: окружение, хопы в порядке исполнения (вход маршрута — token_in первого хопа, выход —
/// token_out последнего), сторону, количество монет, долю keep налога монеты (покупка — налог покупки, продажа —
/// продажи). Возвращает: путь (hops в порядке исполнения, монет, USDT).
/// Ошибки: NoRoute (пусто, налог/крючок забирают всё, шаг не исполним целиком), BadResponse (суммы не в единицы токена).
pub fn eval(
    ctx: &Ctx,
    legs: &[Leg],
    side: Side,
    qty: Decimal,
    tax_keep: Decimal,
) -> GatewayResult<Path> {
    eval_from(ctx, legs, side, qty, tax_keep, None)
}

/// Решение 336: то же, что eval, но обмен хопа, который считается первым (покупка — последний хоп «с конца», продажа —
/// первый хоп), уже посчитан (`first` — итог route_first.rs first: влияние крючка на его входе и обмен — тот же
/// buy_step / sell_step); без него — как eval. Арифметика та же. Ошибки: как eval.
pub fn eval_from(
    ctx: &Ctx,
    legs: &[Leg],
    side: Side,
    qty: Decimal,
    tax_keep: Decimal,
    first: Option<&First>,
) -> GatewayResult<Path> {
    if legs.is_empty() {
        return Err(ctx.err(ErrorCode::NoRoute, "маршрут без пулов"));
    }
    if legs.len()>1 && super::bundles_route::coupled(legs){return super::bundles_route::eval(ctx,legs,side,qty,tax_keep)}
    match side {
        Side::Buy => buy(ctx, legs, qty, tax_keep, first),
        Side::Sell => sell(ctx, legs, qty, tax_keep, first),
    }
}

/// Решение 336 × «335 (лесенка влияния)»: готовый обмен хопа, который считается первым (route_first.rs first), —
/// влияние крючка, применённое на его входе, и сам обмен.
pub type First = (Decimal, Swapped);

/// Что делает: обмен хопа покупки на нужный выход `need_out` (в токене выхода хопа) с долей крючка по его входу
/// (HookCurve::settle, решение «335 (лесенка влияния)»); `tax` — keep налога покупки монеты (последний хоп) или 1.
/// Общий для route_calc.rs buy и route_first.rs first — подбор объёма и финальная котировка не расходятся.
/// Возвращает: (влияние, обмен). Ошибки: NoRoute (налог и крючки забирают всё, влияние не измерено / больше лимита),
/// BadResponse, как pool_quote::swap.
pub(super) fn buy_step(
    ctx: &Ctx,
    leg: &Leg,
    need_out: Decimal,
    tax: Decimal,
) -> GatewayResult<First> {
    let no = |why: String| ctx.err(ErrorCode::NoRoute, why);
    leg.usable.hook.settle(
        leg.usable.hook_pct,
        |pct| {
            let k = tax * keep(pct);
            if k <= Decimal::ZERO {
                return Err(no("налог покупки и крючки забирают всё".into()));
            }
            let pool_out = ceil_units(need_out / k, leg.dec_out).ok_or_else(|| bad(ctx))?;
            let s = pool_quote::swap(ctx, &leg.entry, leg.token_in, false, pool_out)?;
            Ok((s.amount_in, s))
        },
        no,
    )
}

/// Что делает: обмен хопа продажи на точный вход `amount` и доля крючка на этом входе (HookCurve::pct_at). Общий для
/// sell и route_first.rs first. Возвращает: (влияние, обмен). Ошибки: NoRoute (влияние не измерено / больше лимита),
/// как pool_quote::swap.
pub(super) fn sell_step(
    ctx: &Ctx,
    leg: &Leg,
    amount: alloy_primitives::U256,
) -> GatewayResult<First> {
    let pct = leg
        .usable
        .hook
        .pct_at(leg.usable.hook_pct, amount)
        .map_err(|why| ctx.err(ErrorCode::NoRoute, why))?;
    Ok((
        pct,
        pool_quote::swap(ctx, &leg.entry, leg.token_in, true, amount)?,
    ))
}

/// Покупка «с конца»: от нужного количества монеты назад к USDT, каждый пул — точный выход; доля крючка шага — по его
/// входу (buy_step). Решение 336: обмен последнего хопа (считается первым) — готовый, если дан. Ошибки: см. eval.
pub(super) fn buy(
    ctx: &Ctx,
    legs: &[Leg],
    qty: Decimal,
    tax_keep: Decimal,
    first: Option<&First>,
) -> GatewayResult<Path> {
    let n = legs.len();
    let mut swaps: Vec<Option<Swapped>> = vec![None; n];
    let mut pcts = vec![Decimal::ZERO; n];
    // Нужный ВЫХОД текущего хопа (в его токене выхода): у последнего (отдаёт монету) — qty, иначе — вход следующего.
    let mut need_out = qty;
    for i in (0..n).rev() {
        let leg = &legs[i];
        // keep выхода хопа: крючок пула всегда; у последнего хопа (монета) — ещё налог покупки монеты.
        let tax = if i == n - 1 { tax_keep } else { Decimal::ONE };
        let (pct, s) = match first.filter(|_| i == n - 1) {
            Some(f) => f.clone(),
            None => buy_step(ctx, leg, need_out, tax)?,
        };
        need_out = num::to_decimal(s.amount_in, leg.dec_in).ok_or_else(|| bad(ctx))?;
        pcts[i] = pct;
        swaps[i] = Some(s);
    }
    let swaps = complete(ctx, swaps)?;
    let last = &swaps[n - 1];
    let k_last = tax_keep * keep(pcts[n - 1]);
    let coins = (num::to_decimal(last.amount_out, legs[n - 1].dec_out).ok_or_else(|| bad(ctx))?
        * k_last)
        .min(qty);
    let usdt = num::to_decimal(swaps[0].amount_in, legs[0].dec_in).ok_or_else(|| bad(ctx))?;
    build(ctx, legs, swaps, &pcts, (coins, usdt))
}

/// Продажа «с начала»: от количества монеты вперёд к USDT, каждый пул — точный вход; доля крючка шага — по его входу
/// (sell_step). Решение 336: обмен первого хопа — готовый, если дан. Ошибки: см. eval.
fn sell(
    ctx: &Ctx,
    legs: &[Leg],
    qty: Decimal,
    tax_keep: Decimal,
    first: Option<&First>,
) -> GatewayResult<Path> {
    let n = legs.len();
    let mut swaps: Vec<Option<Swapped>> = vec![None; n];
    let mut pcts = vec![Decimal::ZERO; n];
    // В первый пул доходит монета за вычетом налога продажи; дальше — выход предыдущего пула после его крючка.
    let mut amount = num::to_units(qty * tax_keep, legs[0].dec_in).ok_or_else(|| bad(ctx))?;
    let mut usdt = Decimal::ZERO;
    for (i, leg) in legs.iter().enumerate() {
        let (pct, s) = match first.filter(|_| i == 0) {
            Some(f) => f.clone(),
            None => sell_step(ctx, leg, amount)?,
        };
        let got = num::to_decimal(s.amount_out, leg.dec_out).ok_or_else(|| bad(ctx))? * keep(pct);
        if i + 1 < n {
            amount = num::to_units(got, leg.dec_out).ok_or_else(|| bad(ctx))?;
        } else {
            usdt = got;
        }
        pcts[i] = pct;
        swaps[i] = Some(s);
    }
    let swaps = complete(ctx, swaps)?;
    build(ctx, legs, swaps, &pcts, (qty, usdt))
}

/// Все хопы исполнены целиком — вернуть расчёты; иначе NoRoute. Ошибок иных нет.
fn complete(ctx: &Ctx, swaps: Vec<Option<Swapped>>) -> GatewayResult<Vec<Swapped>> {
    let swaps: Vec<Swapped> = swaps.into_iter().flatten().collect();
    if swaps.iter().all(|s| s.complete) {
        Ok(swaps)
    } else {
        Err(ctx.err(ErrorCode::NoRoute, "шаг маршрута не исполним целиком"))
    }
}

/// Путь из хопов в порядке исполнения и итога; у шага — допуск с влиянием крючка, применённым на его входе (решение
/// «335 (лесенка влияния)»). Ошибки: BadResponse (суммы обмена не переводятся в число).
pub(super) fn build(
    ctx: &Ctx,
    legs: &[Leg],
    swaps: Vec<Swapped>,
    pcts: &[Decimal],
    (coins, usdt): (Decimal, Decimal),
) -> GatewayResult<Path> {
    let hops: Vec<Hop> = legs
        .iter()
        .zip(swaps)
        .zip(pcts)
        .map(|((leg, s), pct)| {
            let mut usable = leg.usable.clone();
            usable.hook_pct = *pct;
            hop(
                ctx,
                leg.entry.clone(),
                usable,
                s,
                (leg.token_in, leg.token_out),
                (leg.dec_in, leg.dec_out),
            )
        })
        .collect::<GatewayResult<_>>()?;
    let p=Path {hops,coins,usdt};
    super::temple_budget::check(ctx,std::iter::once(&p))?;
    Ok(p)
}

/// Ошибка перевода суммы в единицы токена. Ошибок нет.
pub(super) fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::BadResponse,
        "сумма не переводится в единицы токена",
    )
}
