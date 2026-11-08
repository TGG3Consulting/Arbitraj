//! Файл: venues/bsc_dex/exec/leg_min.rs
//! Что делает: окончательный минимум выхода каждой доли — последнему шагу доли (он отдаёт выход кошельку). Решение 228:
//! min_k = E_k × max(1 − exec.dex_slippage_pct, B / E), где E_k — свежее ожидаемое доли k (продажа — USDT доли,
//! покупка — монеты доли), E = Σ E_k, B — граница сделки по худшей цене ядра req.worst_price (продажа — Σ монет долей
//! × worst; покупка — Σ USDT долей / worst; это ровно неравенство exec/order.rs::check_price, поэтому B ≤ E). Тогда
//! Σ min_k = max(E × (1 − допуск), B) и ни у одной доли минимум не выше её ожидаемого: доли общего пула, честно
//! посчитанные по порядку (решение 223), больше не откатываются из-за «количество доли × worst» выше их выхода. Одна
//! формула для исполнителя (minOut = сумма), Universal Router и бандла (минимум ноги), умного, прямого пути и сухой
//! сделки. Одна доля — побайтно как раньше (exec/order.rs: наибольшее из B и E × (1 − допуск)).
//! Счёт — в единицах токена (U256), до wei: min_k = наибольшее из ceil(B_u × E_k_u / E_u) (вверх: Σ ≥ B_u; при
//! B_u ≤ E_u каждая ≤ E_k_u; B_u > E_u — отказ) и to_units(E_k × (1 − допуск)), как раньше. Минимум верификации (решения 83, 84)
//! — нижняя граница доли, только если не выше её ожидаемого E_k_u; выше — честный отказ до отправки (цена уехала).
//! Вход точный (exact-in): «максимума входа» нет — вход доли стоит в первом шаге (exec/chain.rs), здесь не меняется.
//! Кто вызывает: exec/{trade_direct, smart_legs, bundle_trade, dry_build}.rs, exec/executor/trade.rs.
//! От чего зависит: exec/chain.rs (Step), quote/path.rs (Path), core/types (TradeRequest, Side), num.rs.
//! Настройки: exec.dex_slippage_pct [bsc_dex].

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::quote::path::Path;
use super::chain::Step;

/// Минимумы долей по решению 228 (см. шапку) — последнему шагу каждой доли (заменяют предварительный минимум
/// exec/order.rs). Принимает: запрос, доли (живой путь, монет доли) в порядке маршрута, шаги долей (exec/chain.rs) в
/// том же порядке, минимумы верификации по долям (None — маршрут без верификации), (decimals монеты, USDT).
/// Вызывать после exec/order.rs::check_price. Ошибки: NotConfigured (exec.dex_slippage_pct), Internal (доли и шаги
/// не совпали), QuoteMismatch (число минимумов верификации; граница сделки выше ожидаемого; минимум верификации выше
/// ожидаемого доли; минимум 0), NoRoute (доля без шагов), BadResponse (число не в единицы токена).
pub fn set_leg_mins<'a>(
    ctx: &Ctx,
    req: &TradeRequest,
    paths: impl IntoIterator<Item = (&'a Path, Decimal)>,
    legs: &mut [Vec<Step>],
    ver: Option<&[Decimal]>,
    (coin_dec, usdt_dec): (u8, u8),
) -> GatewayResult<()> {
    let slip = Decimal::ONE - ctx.dec("exec.dex_slippage_pct")? / Decimal::ONE_HUNDRED;
    let bad = |what: &str| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("{what} не переводится в единицы токена"),
        )
    };
    let units = |v: Decimal, dec: u8, what: &str| num::to_units(v, dec).ok_or_else(|| bad(what));
    let out_dec = if req.side == Side::Buy {
        coin_dec
    } else {
        usdt_dec
    };
    // Ожидаемое доли E_k (десятичное и в единицах), Σ монет и Σ USDT долей — для границы B.
    let (mut exp, mut coins, mut paid) = (Vec::new(), Decimal::ZERO, Decimal::ZERO);
    for (p, qty) in paths {
        let e = if req.side == Side::Buy {
            p.coins
        } else {
            p.usdt
        };
        exp.push((e, units(e, out_dec, "ожидаемый выход доли")?));
        (coins, paid) = (coins + qty, paid + p.usdt);
    }
    if exp.len() != legs.len() {
        return Err(ctx.err(
            ErrorCode::Internal,
            "доли маршрута и их шаги не совпали по числу",
        ));
    }
    if ver.is_some_and(|m| m.len() != legs.len()) {
        let why = "верификация вернула число ног, не равное маршруту";
        return Err(ctx.err(ErrorCode::QuoteMismatch, why));
    }
    let e_sum: Decimal = exp.iter().map(|(e, _)| *e).sum();
    let e_units = exp
        .iter()
        .fold(U256::ZERO, |a, (_, u)| a.saturating_add(*u));
    // Граница сделки: продажа — Σ монет × worst (как qty × worst exec/order.rs), покупка — Σ USDT / worst.
    let b = match req.side {
        Side::Sell => coins * req.worst_price,
        Side::Buy => paid / req.worst_price,
    };
    let b_units = units(b, out_dec, "граница сделки по худшей цене")?;
    // B ≤ E в единицах (check_price проверил то же в десятичных): иначе доля B-части вышла бы выше своего ожидаемого.
    if b_units > e_units || e_units.is_zero() {
        let why = format!(
            "граница сделки {b} по худшей цене {} выше ожидаемого выхода {e_sum} — сделки нет",
            req.worst_price
        );
        return Err(ctx.err(ErrorCode::QuoteMismatch, why));
    }
    for (k, (leg, (e, e_u))) in legs.iter_mut().zip(exp).enumerate() {
        // Доля k: наибольшее из B-части ceil(B_u × E_k_u / E_u) (≤ E_k_u, Σ ≥ B_u; одна доля — ровно B_u) и части
        // допуска to_units(E_k × (1 − допуск)) (≤ E_k_u). Обе пропорциональны E_k — это E_k × max(1 − допуск, B / E)
        // с округлением до wei в безопасную сторону; одна доля — max(B_u, to_units(E × (1 − допуск))) = как раньше.
        let b_part = b_units
            .checked_mul(e_u)
            .ok_or_else(|| bad("граница доли"))?
            .div_ceil(e_units);
        let mut min = b_part.max(units(e * slip, out_dec, "минимум выхода доли")?);
        if let Some(v) = ver.map(|m| m[k]) {
            let v_u = units(v, out_dec, "свежий минимум выхода")?;
            if v_u.is_zero() {
                let why = "свежий минимум выхода 0 — обмен без минимума не отправляется";
                return Err(ctx.err(ErrorCode::QuoteMismatch, why));
            }
            if v_u > e_u {
                let why = format!(
                    "доля {}: минимум верификации {v} выше ожидаемого выхода доли {e} на живом состоянии — цена \
                     уехала, сделки нет",
                    k + 1
                );
                return Err(ctx.err(ErrorCode::QuoteMismatch, why));
            }
            min = min.max(v_u);
        }
        if min.is_zero() {
            let why = format!(
                "доля {}: минимум выхода 0 — обмен без минимума не отправляется",
                k + 1
            );
            return Err(ctx.err(ErrorCode::QuoteMismatch, why));
        }
        let last = leg
            .last_mut()
            .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "доля маршрута без шагов"))?;
        last.order.min_out = min;
    }
    Ok(())
}
