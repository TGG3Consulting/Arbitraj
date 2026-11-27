//! Файл: venues/bsc_dex/quote/side_fit.rs
//! Что делает: решение 393 (задача t178) — «сумма пробы проходит» в пуле по его локальной математике (та же, что у
//! котировки, quote/pool_quote.rs), без порога глубины в полосе цены:
//! - round_trip — пробный круг пробы налогов (check/probe.rs): купить монету на сумму встречного токена и продать назад
//!   остаток (купленное − ½ − ¼, как в пробе), оба обмена исполнимы целиком и выход каждого не меньше min_units;
//! - side / side_usd — одна сторона пула (покупка: встречный → монета; продажа: монеты на ту же сумму по цене пула → встречный):
//!   для сверки с квотером мелкой стороны (check/quoter_one.rs, quoter_amount.rs), графа (pools/graph.rs,
//!   graph_eval.rs) и «слушать ли пул целиком» (pools/depth.rs).
//!
//! Сумма — check.probe_usdt во встречном токене пула (quote/rate.rs, решение 52); не проходит целиком — наибольшая из
//! ряда сумма, сумма/2, сумма/4, … (не больше того, что пул взял на прошлом шаге), которая проходит. Налог пробой
//! меряется как (квотер − пришло) / квотер на ОДНОЙ сумме в одном блоке: кривая пула (проскальзывание) в обеих частях
//! одна и та же и сокращается — налог от глубины пула и от суммы пробы не зависит, кроме округления до 1 единицы
//! токена. Поэтому наименьший выход min_units: одна единица — не больше dex.quoter_max_diff_pct % выхода (тот же допуск
//! «наш расчёт против сети», что у сверки с квотером): меньший выход дал бы ложный «налог 0» или ложный налог.
//! Кто вызывает: check/taxes_util.rs, check/tax_deep.rs, check/probe_tiny.rs, check/quoter_one.rs,
//! check/quoter_side.rs, check/quoter_amount.rs, pools/graph.rs, pools/graph_eval.rs, pools/depth.rs; тесты.
//! От чего зависит: quote/pool_quote.rs, quote/pool_after.rs, quote/spot.rs, quote/rate.rs, quote/emptiness.rs,
//! pools/registry.rs, tokens.rs, num.rs.
//! Настройки: check.probe_usdt, dex.quoter_max_diff_pct [bsc_dex].

use alloy_primitives::U256;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::core::types::Side;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::registry::{Entry, Registry};
use super::super::tokens::Tokens;
use super::{emptiness, pool_after, pool_quote, rate, spot};

/// Наименьший выход обмена пробы и сверки мелкой стороны, минимальные единицы (см. шапку). Ошибки: NotConfigured —
/// dex.quoter_max_diff_pct не задана или не больше 0.
pub fn min_units(ctx: &Ctx) -> GatewayResult<U256> {
    let pct = ctx.dec("dex.quoter_max_diff_pct")?;
    min_units_for(pct).ok_or_else(|| {
        ctx.not_set(
            "dex.quoter_max_diff_pct",
            "dex.quoter_max_diff_pct должна быть больше 0 — от неё наименьший выход пробного обмена (решение 393)",
        )
    })
}

/// ⌈100 / допуск⌉ единиц: при таком выходе ошибка округления в 1 единицу — не больше допуска, %. None — допуск не больше
/// 0 или число не помещается. Ошибок нет.
pub fn min_units_for(pct: Decimal) -> Option<U256> {
    if pct <= Decimal::ZERO {
        return None;
    }
    let n = (Decimal::ONE_HUNDRED / pct).ceil().to_u128()?;
    Some(U256::from(n.max(1)))
}

/// Сумма пробы check.probe_usdt во встречном токене пула (минимальные единицы). Принимает: окружение, реестр, USDT, его
/// знаки, пул. Ошибки: NotConfigured (check.probe_usdt), NoRoute (нет курса промежуточной монеты, quote/rate.rs).
pub fn cap(
    ctx: &Ctx,
    reg: &Registry,
    usdt: alloy_primitives::Address,
    usdt_dec: u8,
    e: &Entry,
) -> GatewayResult<U256> {
    let probe = num::to_units(ctx.dec("check.probe_usdt")?, usdt_dec).ok_or_else(|| {
        ctx.not_set(
            "check.probe_usdt",
            "check.probe_usdt не переводится в единицы USDT",
        )
    })?;
    rate::from_usdt(ctx, reg, usdt, e.usdt, probe)
}

/// Наибольшая сумма из ряда cap, cap/2, … с итогом `f`. `f` возвращает: Ok(итог) — сумма проходит; Err(Some(x)) —
/// не проходит, пул взял не больше x (следующая сумма — не больше x); Err(None) — меньшие суммы не помогут (выход уже
/// меньше наименьшего или расчёт невозможен). Ошибок нет.
pub fn halve<T>(cap: U256, mut f: impl FnMut(U256) -> Result<T, Option<U256>>) -> Option<T> {
    let mut a = cap;
    while !a.is_zero() {
        match f(a) {
            Ok(t) => return Some(t),
            Err(None) => return None,
            Err(Some(took)) => a = (a >> 1usize).min(took),
        }
    }
    None
}

/// Пробный круг: сумма встречного токена в пул и купленные монеты.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trip {
    pub amount: U256,
    pub coins: U256,
}

/// Пробный круг пробы налогов (см. шапку). Продажа остатка — на состоянии пула после покупки, где оно считается точно
/// (V2, V3 — quote/pool_after.rs), иначе — на текущем (строже: у пула с ликвидностью только с одной стороны цены
/// круг не пройдёт). Принимает: окружение, пул, наибольшую сумму, наименьший выход. Возвращает: None — не проходит ни
/// на одной сумме ряда или пул пуст. Ошибок нет.
pub fn round_trip(ctx: &Ctx, e: &Entry, cap: U256, min: U256) -> Option<Trip> {
    if emptiness::empty(e) {
        return None;
    }
    // Решение 410 (t195): копия записи для состояния после покупки — одна на круг (создаётся при первой продаже по
    // состоянию после покупки, дальше меняется только её состояние), а не на каждую сумму ряда.
    let mut after_copy: Option<Entry> = None;
    halve(cap, |a| {
        let buy = pool_quote::swap(ctx, e, e.usdt, true, a).map_err(|_| None)?;
        if !buy.complete {
            return Err(Some(buy.amount_in));
        }
        if buy.amount_out < min {
            return Err(None);
        }
        let b = buy.amount_out;
        // Как в пробе (check/probe.rs): половина и четверть остаются, продаётся остаток.
        let rest = b - b / U256::from(2u8) - b / U256::from(4u8);
        let sell = if pool_after::appliable(e) {
            let (_, after) = pool_after::apply(ctx, e, e.usdt, a).map_err(|_| None)?;
            let x = after_copy.get_or_insert_with(|| e.clone());
            x.pool.state = after;
            pool_quote::swap(ctx, x, e.coin, true, rest)
        } else {
            pool_quote::swap(ctx, e, e.coin, true, rest)
        };
        match sell {
            Ok(s) if s.complete && s.amount_out >= min => Ok(Trip {
                amount: a,
                coins: b,
            }),
            // Продать нечем совсем (обратной стороны в карте нет) — меньшие суммы не помогут.
            Ok(s) if !s.complete && s.amount_in.is_zero() => Err(None),
            // Остаток не продаётся целиком или выход мал — на меньшей покупке остаток меньше.
            Ok(_) => Err(Some(a)),
            Err(_) => Err(None),
        }
    })
}

/// Проходит ли сторона пула (см. шапку): наибольшая сумма встречного токена из ряда. Покупка — встречный в пул целиком,
/// монет не меньше min; продажа — монеты на эту сумму по цене пула (quote/spot.rs) не меньше min, продаются целиком,
/// выход не меньше min. Принимает: окружение, пул, сторону, наибольшую сумму, наименьший выход. Возвращает: сумму
/// встречного токена; None — не проходит или пул пуст. Ошибок нет.
pub fn side(ctx: &Ctx, e: &Entry, s: Side, cap: U256, min: U256) -> Option<U256> {
    if emptiness::empty(e) {
        return None;
    }
    halve(cap, |a| {
        let (token_in, amount) = match s {
            Side::Buy => (e.usdt, a),
            Side::Sell => match spot::coins_for(e, a) {
                Some(c) if c >= min => (e.coin, c),
                _ => return Err(None),
            },
        };
        let r = pool_quote::swap(ctx, e, token_in, true, amount).map_err(|_| None)?;
        if !r.complete {
            // Пул не взял ничего — в эту сторону ликвидности в карте нет, меньшие суммы не помогут. Продажа: пул взял
            // часть монет — следующая сумма вдвое меньше; покупка — не больше взятого.
            if r.amount_in.is_zero() {
                return Err(None);
            }
            return Err(Some(if s == Side::Buy { r.amount_in } else { a }));
        }
        if r.amount_out < min {
            return Err(None);
        }
        Ok(a)
    })
}

/// Сторона пула проходит на сумме пробы (side) — с чтением настроек и USDT. Принимает: окружение, реестр, токены, знаки
/// USDT, пул, сторону. Возвращает: (сумма встречного токена, доля 1/2^k от check.probe_usdt в USDT); None — не проходит,
/// не задана настройка, нет курса. Ошибок нет.
pub fn side_usd(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt_dec: u8,
    e: &Entry,
    s: Side,
) -> Option<(U256, Decimal)> {
    let usdt = tokens.usdt(ctx).ok()?;
    let cap = cap(ctx, reg, usdt, usdt_dec, e).ok()?;
    let min = min_units(ctx).ok()?;
    let a = side(ctx, e, s, cap, min).or_else(|| super::brokerbot_size::one(e,s,cap))?;
    // Доля суммы пробы: a — cap, сдвинутая вправо, или взятое пулом; в USDT — пропорцией (курс тот же, что у cap).
    let probe = ctx.dec("check.probe_usdt").ok()?;
    let (an, cn) = (num::to_decimal(a, 0)?, num::to_decimal(cap, 0)?);
    if cn.is_zero() {
        return None;
    }
    Some((a, probe * (an / cn)))
}
