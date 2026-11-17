//! Файл: venues/bsc_dex/exec/dry_fund_gather.rs
//! Что делает: решение 380 (задача t166) — чистая математика набора источников монет для пополнения кошелька в
//! симуляции (exec/dry_fund.rs): раньше брался ОДИН источник с балансом ≥ суммы перевода, и если такого не было —
//! симуляция не запускалась, а предпроверка писала «продажа не пройдёт» (живой бот 17.11, 06:20–09:52 UTC: 350 отказов,
//! AVA bitget → bsc_dex 242, FLOKI mexc → bsc_dex 71, NXT mexc → bsc_dex 32, YCT gate → bsc_dex 3, MIRA bingx → bsc_dex 2).
//! Теперь:
//! 1. как раньше — сначала один источник: наибольший пул V2/V3 вне маршрута с балансом ≥ суммы, затем (при
//!    presell.fund_fallback = singleton) наибольший годный держатель-синглтон с балансом ≥ суммы — тот же выбор, что
//!    до решения 380, поэтому прежние удачные симуляции не меняются;
//! 2. одного не хватает — несколько источников вместе (по убыванию баланса, последний — на остаток): каждый перевод
//!    от источника, годного и поодиночке (пулы вне маршрута; держатель — только годный при маршруте, route_ok), значит
//!    и сумма переводов цену маршрута не меняет; излишек пары reflection-токена меряется вокруг ВСЕХ переводов
//!    (exec/dry_surplus.rs around);
//! 3. и вместе не хватает — Err(сколько есть у всех источников): по нему адаптер считает, какой объём продажи
//!    проверить МОЖНО (net), а ядро пересчитывает сделку на объём не больше этого (core/precheck_dex_cut.rs).
//!
//! Кто вызывает: exec/dry_fund.rs (plan).
//! От чего зависит: alloy_primitives, num.rs (перевод U256 ↔ Decimal), core::types::TokenTaxes.
//! Настройки: нет.

use alloy_primitives::{Address, U256};
use rust_decimal::{Decimal, RoundingStrategy};

use crate::core::types::TokenTaxes;

use super::super::num;

/// Набор переводов: (источник, сколько взять). Принимает: группы кандидатов (адрес, баланс) в порядке старшинства
/// (пулы, затем держатели), сумму. Возвращает: Ok(переводы, сумма ровно `need`) или Err(всего у всех источников, без
/// повторов адреса) — набрать нельзя. Ошибок нет.
pub fn gather(groups: &[&[(Address, U256)]], need: U256) -> Result<Vec<(Address, U256)>, U256> {
    if need.is_zero() {
        return Ok(Vec::new());
    }
    // Шаг 1 (как до решения 380): один источник — наибольший в первой группе, где он ≥ суммы.
    for g in groups {
        if let Some(&(a, _)) = g.iter().filter(|(_, b)| *b >= need).max_by_key(|(_, b)| *b) {
            return Ok(vec![(a, need)]);
        }
    }
    // Шаг 2: все источники вместе, по убыванию баланса; один адрес — один раз.
    let mut all: Vec<(Address, U256)> = Vec::new();
    for &(a, b) in groups.iter().flat_map(|g| g.iter()) {
        if !b.is_zero() && !all.iter().any(|(x, _)| *x == a) {
            all.push((a, b));
        }
    }
    all.sort_by_key(|x| std::cmp::Reverse(x.1));
    let mut left = need;
    let mut out = Vec::new();
    for (a, b) in &all {
        if left.is_zero() {
            break;
        }
        let take = (*b).min(left);
        out.push((*a, take));
        left -= take;
    }
    if left.is_zero() {
        Ok(out)
    } else {
        Err(all
            .iter()
            .fold(U256::ZERO, |s, (_, b)| s.saturating_add(*b)))
    }
}

/// Сколько дойдёт до кошелька из `total` переводом с налогом токена (обратное к dry_fund::gross): total × (1 − налог),
/// вниз до целой единицы; налог — наибольший из налога покупки и перевода, как в gross. Принимает: сумму у источников,
/// налоги (None — без налога). Возвращает: единицы; None — налог ≥ 100 % или сумма не переводится в число. Ошибок нет.
pub fn net(total: U256, taxes: Option<&TokenTaxes>) -> Option<U256> {
    let pct = taxes.map_or(Decimal::ZERO, |t| t.buy_pct.max(t.transfer_pct));
    if pct.is_zero() {
        return Some(total);
    }
    let keep = Decimal::ONE - pct / Decimal::ONE_HUNDRED;
    if keep <= Decimal::ZERO {
        return None;
    }
    let d = num::to_decimal(total, 0)?;
    num::to_units((d * keep).floor(), 0)
}

/// Объём продажи (в единицах запроса), который можно проверить: qty × можно / нужно, вниз до точности монеты, не больше
/// qty. Принимает: количество запроса (монеты), сколько единиц входа можно дать кошельку, сколько нужно, знаков монеты
/// (decimals токена с узла). Возвращает: объём; None — нужно 0 или числа не переводятся. Ошибок нет.
pub fn qty_share(qty: Decimal, can: U256, need: U256, decimals: u8) -> Option<Decimal> {
    let (c, n) = (num::to_decimal(can, 0)?, num::to_decimal(need, 0)?);
    if n.is_zero() {
        return None;
    }
    // Сначала qty × можно, потом / нужно — точнее (доля не округляется до умножения); переполнение Decimal на больших
    // единицах — тогда qty × (можно / нужно). Вниз до знаков монеты: округление не даст объём больше проверяемого.
    let raw = qty
        .checked_mul(c)
        .and_then(|x| x.checked_div(n))
        .unwrap_or_else(|| qty * (c / n));
    let share = raw.round_dp_with_strategy(u32::from(decimals), RoundingStrategy::ToZero);
    Some(share.min(qty))
}
