//! Файл: venues/bsc_dex/quote/smart.rs
//! Что делает: поиск «умного» маршрута (решение 38) — деление объёма между пулами монеты с USDT у PancakeSwap
//! (V2, V3, Infinity CL, Infinity Bin) и путями через промежуточные монеты: объём режется на шаги по smart.split_step_pct, каждый шаг отдаётся пулу,
//! где он выгоднее всего с учётом уже отданных ему шагов (покупка — меньше USDT за шаг, продажа — больше),
//! не больше smart.max_splits пулов. Расчёт доли в пуле — тот же, что у прямого режима (quote/direct.rs::calc).
//! Монета с налогом (покупки, продажи или перевода) — только пулы V2: в V3 и Infinity вход с налогом откатывается,
//! а минимум выхода проверяется не по факту (research/flow/bsc_smart_router.md, раздел 4.1).
//! Кто вызывает: quote/smart_quote.rs.
//! От чего зависит: quote/direct.rs, quote/via.rs, quote/path.rs, quote/select.rs, pools/registry.rs, pools/families.rs.
//! Настройки: smart.max_splits, smart.split_step_pct [bsc_dex]; через вызываемые — см. их шапки.
//!
//! Только пулы PancakeSwap: Universal Router PancakeSwap вычисляет адреса пар V2 и пулов V3 только по фабрикам
//! PancakeSwap — research/flow/bsc_smart_router.md, раздел 1.4 (V2SwapRouter.sol#L60-L62, V3SwapRouter.sol#L152-L156).
//! Доля через промежуточную монету (решение 52): по каждой промежуточной — одна лучшая пара пулов PancakeSwap
//! («промежуточная — USDT» и «монета — промежуточная», quote/via.rs) как отдельный кандидат деления (у двух долей
//! через одну промежуточную общий пул P1 — их ликвидность не независима, поэтому пара одна).
//! Шаги долей — как у официального маршрутизатора (доли входа с шагом в процентах) — там же, раздел 3.1.

use rust_decimal::Decimal;

use crate::core::types::{Side, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::path::Path;
use super::smart_ways::{Slot, calc, pairs, pools};
use super::via::Via;

/// Умный маршрут: доли объёма по пулам PancakeSwap и путям через промежуточные. Принимает: расчёт путей, тикер,
/// количество, пулы монеты с USDT, промежуточные с decimals. Возвращает: доли (пути).
/// Ошибки: NotConfigured (smart.*), NoRoute (нет пригодного кандидата — с причинами).
pub fn best(
    v: &Via<'_>,
    ticker: &str,
    qty: Decimal,
    entries: &[Entry],
    mids: &[(Mid, u8)],
) -> GatewayResult<Vec<Path>> {
    let ctx = &v.q.ctx;
    let max_splits = usize::try_from(ctx.int("smart.max_splits")?).unwrap_or(usize::MAX);
    let step_pct = ctx.dec("smart.split_step_pct")?;
    if step_pct <= Decimal::ZERO || step_pct > Decimal::ONE_HUNDRED || max_splits == 0 {
        return Err(ctx.not_set(
            "smart.split_step_pct",
            "smart.split_step_pct должен быть в (0; 100], smart.max_splits — больше нуля",
        ));
    }
    let t: &TokenTaxes = v.taxes;
    let taxed = !(t.buy_pct.is_zero() && t.sell_pct.is_zero() && t.transfer_pct.is_zero());
    let (mut ways, mut why) = pools(v.q, entries, v.side, taxed, v.usdt_dec);
    ways.extend(pairs(v, mids, qty, &mut why));
    let mut slots: Vec<Slot> = ways
        .into_iter()
        .map(|way| Slot {
            way,
            qty: Decimal::ZERO,
            got: None,
        })
        .collect();
    let unit = qty * step_pct / Decimal::ONE_HUNDRED;
    let mut left = qty;
    while left > Decimal::ZERO {
        let step = unit.min(left);
        let used = slots.iter().filter(|s| s.got.is_some()).count();
        let mut pick: Option<(usize, Path, Decimal)> = None;
        for (i, s) in slots.iter().enumerate() {
            if s.got.is_none() && used >= max_splits {
                continue;
            }
            let Ok(p) = calc(v, &s.way, s.qty + step) else {
                continue;
            };
            let before = s.got.as_ref().map_or(Decimal::ZERO, |c| c.usdt);
            let delta = p.usdt - before;
            let better = pick.as_ref().is_none_or(|(_, _, d)| match v.side {
                Side::Buy => delta < *d,
                Side::Sell => delta > *d,
            });
            if better {
                pick = Some((i, p, delta));
            }
        }
        // Ни один кандидат не принимает шаг целиком — дальше объём не делится (исполнимое меньше запрошенного).
        let Some((i, p, _)) = pick else { break };
        slots[i].qty += step;
        slots[i].got = Some(p);
        left -= step;
    }
    let legs: Vec<Path> = slots.into_iter().filter_map(|s| s.got).collect();
    if legs.is_empty() {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            format!(
                "{ticker}: умный маршрут — нет пригодного пула PancakeSwap ({})",
                why.join("; ")
            ),
        ));
    }
    Ok(legs)
}
