//! Файл: venues/bsc_dex/quote/smart.rs
//! Что делает: поиск «умного» маршрута (решение 38) — деление объёма между пулами монеты с USDT у PancakeSwap
//! (V2, V3, Infinity CL, Infinity Bin): объём режется на шаги по smart.split_step_pct, каждый шаг отдаётся пулу,
//! где он выгоднее всего с учётом уже отданных ему шагов (покупка — меньше USDT за шаг, продажа — больше),
//! не больше smart.max_splits пулов. Расчёт доли в пуле — тот же, что у прямого режима (quote/direct.rs::calc).
//! Монета с налогом (покупки, продажи или перевода) — только пулы V2: в V3 и Infinity вход с налогом откатывается,
//! а минимум выхода проверяется не по факту (research/flow/bsc_smart_router.md, раздел 4.1).
//! Кто вызывает: quote/smart_quote.rs.
//! От чего зависит: quote/direct.rs, quote/select.rs, pools/registry.rs, pools/families.rs.
//! Настройки: smart.max_splits, smart.split_step_pct [bsc_dex]; через вызываемые — см. их шапки.
//!
//! Только пулы PancakeSwap: Universal Router PancakeSwap вычисляет адреса пар V2 и пулов V3 только по фабрикам
//! PancakeSwap — research/flow/bsc_smart_router.md, раздел 1.4 (V2SwapRouter.sol#L60-L62, V3SwapRouter.sol#L152-L156).
//! Хопов через промежуточные монеты нет: реестр знает только пулы «монета — USDT» (каталог настроек не задаёт
//! промежуточные монеты) — smart.max_hops пока не используется, запрос на изменение стыка в отчёте шага 1e.
//! Шаги долей — как у официального маршрутизатора (доли входа с шагом в процентах) — там же, раздел 3.1.

use rust_decimal::Decimal;

use crate::core::types::{PoolKind, Side, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::pools::families::PANCAKESWAP;
use super::super::pools::registry::Entry;
use super::Quoting;
use super::direct::{self, Candidate};
use super::select::{self, Usable};

/// Пул-кандидат и сколько монет ему уже отдано.
struct Slot {
    entry: Entry,
    usable: Usable,
    qty: Decimal,
    got: Option<Candidate>,
}

/// Пулы, пригодные для умного маршрута. Возвращает: пары (пул, допуск) и причины отказа по остальным.
fn pools(
    q: &Quoting,
    entries: &[Entry],
    side: Side,
    taxed: bool,
    usdt_dec: u8,
) -> (Vec<(Entry, Usable)>, Vec<String>) {
    let mut ok = Vec::new();
    let mut why = Vec::new();
    for e in entries.iter().filter(|e| e.family.dex == PANCAKESWAP) {
        if taxed && e.pool.kind != PoolKind::V2 {
            why.push(format!(
                "{}: у монеты налог — в умном маршруте только V2",
                e.family.title
            ));
            continue;
        }
        match select::usable(&q.ctx, &q.checks, e, side, usdt_dec) {
            Ok(u) => ok.push((e.clone(), u)),
            Err(w) => why.push(format!("{}: {w}", e.family.title)),
        }
    }
    (ok, why)
}

/// Умный маршрут: доли объёма по пулам PancakeSwap. Принимает: окружение, тикер, сторону, количество,
/// пулы монеты, налоги, (decimals монеты, USDT). Возвращает: доли (у каждой — свой пул, количество монет и USDT).
/// Ошибки: NotConfigured (smart.*), NoRoute (нет пригодного пула PancakeSwap — с причинами).
#[allow(clippy::too_many_arguments)]
pub fn best(
    q: &Quoting,
    ticker: &str,
    side: Side,
    qty: Decimal,
    entries: &[Entry],
    t: &TokenTaxes,
    decs: (u8, u8),
) -> GatewayResult<Vec<Candidate>> {
    let ctx = &q.ctx;
    let max_splits = usize::try_from(ctx.int("smart.max_splits")?).unwrap_or(usize::MAX);
    let step_pct = ctx.dec("smart.split_step_pct")?;
    if step_pct <= Decimal::ZERO || step_pct > Decimal::ONE_HUNDRED || max_splits == 0 {
        return Err(ctx.err(
            ErrorCode::NotConfigured,
            "smart.split_step_pct должен быть в (0; 100], smart.max_splits — больше нуля",
        ));
    }
    let taxed = !(t.buy_pct.is_zero() && t.sell_pct.is_zero() && t.transfer_pct.is_zero());
    let (list, why) = pools(q, entries, side, taxed, decs.1);
    let mut slots: Vec<Slot> = list
        .into_iter()
        .map(|(entry, usable)| Slot {
            entry,
            usable,
            qty: Decimal::ZERO,
            got: None,
        })
        .collect();
    let unit = qty * step_pct / Decimal::ONE_HUNDRED;
    let mut left = qty;
    while left > Decimal::ZERO {
        let step = unit.min(left);
        let used = slots.iter().filter(|s| s.got.is_some()).count();
        let mut pick: Option<(usize, Candidate, Decimal)> = None;
        for (i, s) in slots.iter().enumerate() {
            if s.got.is_none() && used >= max_splits {
                continue;
            }
            let Ok((swapped, coins, usdt)) =
                direct::calc(ctx, &s.entry, &s.usable, side, s.qty + step, t, decs)
            else {
                continue;
            };
            if !swapped.complete {
                continue;
            }
            let before = s.got.as_ref().map_or(Decimal::ZERO, |c| c.usdt);
            let delta = usdt - before;
            let better = pick.as_ref().is_none_or(|(_, _, d)| match side {
                Side::Buy => delta < *d,
                Side::Sell => delta > *d,
            });
            if better {
                let c = Candidate {
                    entry: s.entry.clone(),
                    usable: s.usable.clone(),
                    swapped,
                    coins,
                    usdt,
                };
                pick = Some((i, c, delta));
            }
        }
        // Ни один пул не принимает шаг целиком — дальше объём не делится (исполнимое меньше запрошенного).
        let Some((i, c, _)) = pick else { break };
        slots[i].qty += step;
        slots[i].got = Some(c);
        left -= step;
    }
    let legs: Vec<Candidate> = slots.into_iter().filter_map(|s| s.got).collect();
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
