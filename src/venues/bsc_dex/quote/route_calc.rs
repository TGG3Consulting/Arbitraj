//! Файл: venues/bsc_dex/quote/route_calc.rs
//! Что делает: расчёт пути обмена из любого числа последовательных пулов (хопов) — обобщение двухшагового пути
//! через промежуточную монету (решение 52, quote/via.rs) до маршрута глубины 2–3 будущего графа (решение 80).
//! Математика и порядок операций — ровно те же, что в quote/via.rs, только для N хопов. Покупка решается «с конца»
//! (от нужного количества монеты назад к USDT, каждый пул отдаёт точный выход), продажа — «с начала» (от количества
//! монеты вперёд к USDT, каждый пул принимает точный вход).
//! Налог монеты применяется на стороне монеты (покупка — к выходу последнего хопа, продажа — ко входу первого),
//! крючок каждого пула — к его выходу. Шаг, не исполнимый целиком в прочитанных пределах, — маршрута нет (NoRoute).
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
    if legs.is_empty() {
        return Err(ctx.err(ErrorCode::NoRoute, "маршрут без пулов"));
    }
    match side {
        Side::Buy => buy(ctx, legs, qty, tax_keep),
        Side::Sell => sell(ctx, legs, qty, tax_keep),
    }
}

/// Покупка «с конца»: от нужного количества монеты назад к USDT, каждый пул — точный выход. Ошибки: см. eval.
fn buy(ctx: &Ctx, legs: &[Leg], qty: Decimal, tax_keep: Decimal) -> GatewayResult<Path> {
    let n = legs.len();
    let mut swaps: Vec<Option<Swapped>> = vec![None; n];
    // Нужный ВЫХОД текущего хопа (в его токене выхода): у последнего (отдаёт монету) — qty, иначе — вход следующего.
    let mut need_out = qty;
    for i in (0..n).rev() {
        let leg = &legs[i];
        // keep выхода хопа: крючок пула всегда; у последнего хопа (монета) — ещё налог покупки монеты.
        let k = if i == n - 1 {
            tax_keep * keep(leg.usable.hook_pct)
        } else {
            keep(leg.usable.hook_pct)
        };
        if k <= Decimal::ZERO {
            return Err(ctx.err(ErrorCode::NoRoute, "налог покупки и крючки забирают всё"));
        }
        let pool_out = ceil_units(need_out / k, leg.dec_out).ok_or_else(|| bad(ctx))?;
        let s = pool_quote::swap(ctx, &leg.entry, leg.token_in, false, pool_out)?;
        need_out = num::to_decimal(s.amount_in, leg.dec_in).ok_or_else(|| bad(ctx))?;
        swaps[i] = Some(s);
    }
    let swaps = complete(ctx, swaps)?;
    let last = &swaps[n - 1];
    let k_last = tax_keep * keep(legs[n - 1].usable.hook_pct);
    let coins = (num::to_decimal(last.amount_out, legs[n - 1].dec_out).ok_or_else(|| bad(ctx))?
        * k_last)
        .min(qty);
    let usdt = num::to_decimal(swaps[0].amount_in, legs[0].dec_in).ok_or_else(|| bad(ctx))?;
    build(ctx, legs, swaps, coins, usdt)
}

/// Продажа «с начала»: от количества монеты вперёд к USDT, каждый пул — точный вход. Ошибки: см. eval.
fn sell(ctx: &Ctx, legs: &[Leg], qty: Decimal, tax_keep: Decimal) -> GatewayResult<Path> {
    let n = legs.len();
    let mut swaps: Vec<Option<Swapped>> = vec![None; n];
    // В первый пул доходит монета за вычетом налога продажи; дальше — выход предыдущего пула после его крючка.
    let mut amount = num::to_units(qty * tax_keep, legs[0].dec_in).ok_or_else(|| bad(ctx))?;
    let mut usdt = Decimal::ZERO;
    for (i, leg) in legs.iter().enumerate() {
        let s = pool_quote::swap(ctx, &leg.entry, leg.token_in, true, amount)?;
        let got = num::to_decimal(s.amount_out, leg.dec_out).ok_or_else(|| bad(ctx))?
            * keep(leg.usable.hook_pct);
        if i + 1 < n {
            amount = num::to_units(got, leg.dec_out).ok_or_else(|| bad(ctx))?;
        } else {
            usdt = got;
        }
        swaps[i] = Some(s);
    }
    let swaps = complete(ctx, swaps)?;
    build(ctx, legs, swaps, qty, usdt)
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

/// Путь из хопов в порядке исполнения и итога. Ошибки: BadResponse (суммы обмена не переводятся в число).
fn build(
    ctx: &Ctx,
    legs: &[Leg],
    swaps: Vec<Swapped>,
    coins: Decimal,
    usdt: Decimal,
) -> GatewayResult<Path> {
    let hops: Vec<Hop> = legs
        .iter()
        .zip(swaps)
        .map(|(leg, s)| {
            hop(
                ctx,
                leg.entry.clone(),
                leg.usable.clone(),
                s,
                (leg.token_in, leg.token_out),
                (leg.dec_in, leg.dec_out),
            )
        })
        .collect::<GatewayResult<_>>()?;
    Ok(Path { hops, coins, usdt })
}

/// Ошибка перевода суммы в единицы токена. Ошибок нет.
fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::BadResponse,
        "сумма не переводится в единицы токена",
    )
}
