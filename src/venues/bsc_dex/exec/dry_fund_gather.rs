//! Файл: venues/bsc_dex/exec/dry_fund_gather.rs
//! Что делает: решение 380 (задача t166) — чистая математика набора источников монет для пополнения кошелька в
//! симуляции (exec/dry_fund.rs): раньше брался ОДИН источник с балансом ≥ суммы перевода, и если такого не было —
//! симуляция не запускалась, а предпроверка писала «продажа не пройдёт» (живой бот 17.11, 06:20–09:52 UTC: 350 отказов,
//! AVA bitget → bsc_dex 242, FLOKI mexc → bsc_dex 71, NXT mexc → bsc_dex 32, YCT gate → bsc_dex 3, MIRA bingx → bsc_dex 2).
//! Теперь:
//! 1. как раньше — сначала один источник: наибольший пул V2/V3 вне маршрута с балансом ≥ суммы, затем (при
//!    presell.fund_fallback = singleton) наибольший годный держатель-синглтон с балансом ≥ суммы — тот же выбор, что
//!    до решения 380, поэтому прежние удачные симуляции не меняются;
//! 2. одного не хватает — несколько источников вместе (ревью t166, Н4): источник с наибольшим балансом — «узел»,
//!    остальные переводят ему свои доли (налог перевода между ними — наибольший из налогов токена), а узел ОДНИМ
//!    переводом отдаёт кошельку всю сумму, как придёт настоящий перевод с биржи: лимит размера перевода у токена
//!    (max tx / max wallet) проверяется на полной сумме, мягче проверка не становится. Каждый источник годен и
//!    поодиночке (пулы вне маршрута; держатель — только годный при маршруте, route_ok; приход на узел — тот же довод:
//!    пул вне маршрута, у держателя баланс читают только sync / settle), значит и переводы цену маршрута не меняют;
//!    излишек пары reflection-токена меряется вокруг ВСЕХ переводов (exec/dry_surplus.rs around);
//! 3. и вместе не хватает — Err(сколько узел смог бы перевести кошельку): по нему адаптер считает, какой объём продажи
//!    проверить МОЖНО (net, с запасом на округления — exec/dry_fund.rs), а ядро пересчитывает сделку на объём не
//!    больше этого (core/precheck_dex_cut.rs).
//!
//! Кто вызывает: exec/dry_fund.rs (plan).
//! От чего зависит: alloy_primitives, num.rs (перевод U256 ↔ Decimal), core::types::TokenTaxes.
//! Настройки: нет.

use alloy_primitives::{Address, U256};
use rust_decimal::{Decimal, RoundingStrategy};

use crate::core::types::TokenTaxes;

use super::super::num;

/// Перевод-пополнение внутри симуляции: откуда, куда (None — кошелёк сделки; Some — узел набора), сколько, что это
/// за источник (для журнала: «пула» или «держателя-синглтона (…)»).
pub struct Fund {
    pub src: Address,
    pub to: Option<Address>,
    pub amount: U256,
    pub what: String,
}

/// Текст пополнения для журнала: один источник — как до решения 380 («N единиц из пула 0x…»); узел — «N единиц одним
/// переводом из пула 0x… (узел набора; ему переведено: a из пула 0x…; b из держателя-синглтона (…) 0x…)».
/// Принимает: переводы (последний — на кошелёк). Ошибок нет.
pub fn text(fs: &[Fund]) -> String {
    let fmt = |f: &Fund| format!("{} из {} {:#x}", f.amount, f.what, f.src);
    match fs.split_last() {
        Some((last, [])) => format!("{} единиц из {} {:#x}", last.amount, last.what, last.src),
        Some((last, feeds)) => format!(
            "{} единиц одним переводом из {} {:#x} (узел набора, решение 380; ему переведено: {})",
            last.amount,
            last.what,
            last.src,
            feeds.iter().map(fmt).collect::<Vec<_>>().join("; ")
        ),
        None => String::new(),
    }
}

/// Как пополнить кошелёк: одним переводом от одного источника или через «узел» (ревью t166, Н4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// Один источник с полной суммой — перевод сразу на кошелёк (как до решения 380).
    Direct(Address),
    /// Несколько источников: остальные переводят свои доли источнику с наибольшим балансом (узлу), а он — ОДНИМ
    /// переводом всю сумму на кошелёк, как придёт настоящий перевод с биржи (лимит размера перевода у токена — max tx
    /// / max wallet — проверяется на полной сумме, а не на мелких долях).
    Relay {
        hub: Address,
        /// (источник, сколько переводит узлу).
        feeds: Vec<(Address, U256)>,
    },
}

/// Набор источников. Принимает: группы кандидатов (адрес, баланс) в порядке старшинства (пулы, затем держатели),
/// сумму, которую узел/источник переводит кошельку (> 0), налоги токена (перевод между источниками — с наибольшим из
/// налогов покупки, продажи и перевода: токен может считать его любым из них). Возвращает: Ok(Pick) или Err(сколько
/// узел смог бы перевести кошельку: свой баланс + что дойдёт от остальных) — набрать нельзя. Ошибок нет.
pub fn gather(
    groups: &[&[(Address, U256)]],
    need: U256,
    taxes: Option<&TokenTaxes>,
) -> Result<Pick, U256> {
    // Шаг 1 (как до решения 380): один источник — наибольший в первой группе, где он ≥ суммы.
    for g in groups {
        if let Some(&(a, _)) = g.iter().filter(|(_, b)| *b >= need).max_by_key(|(_, b)| *b) {
            return Ok(Pick::Direct(a));
        }
    }
    // Шаг 2: все источники вместе, по убыванию баланса; один адрес — один раз; узел — наибольший.
    let mut all: Vec<(Address, U256)> = Vec::new();
    for &(a, b) in groups.iter().flat_map(|g| g.iter()) {
        if !b.is_zero() && !all.iter().any(|(x, _)| *x == a) {
            all.push((a, b));
        }
    }
    all.sort_by_key(|x| std::cmp::Reverse(x.1));
    let Some(&(hub, own)) = all.first() else {
        return Err(U256::ZERO);
    };
    let others = &all[1..];
    let sum = others
        .iter()
        .fold(U256::ZERO, |s, (_, b)| s.saturating_add(*b));
    let reach = own.saturating_add(scale(sum, hop_keep(taxes), false).unwrap_or(U256::ZERO));
    // Сколько остальные должны отправить узлу, чтобы после налога перевода у него было ≥ суммы.
    let Some(mut left) = scale(need.saturating_sub(own), hop_keep(taxes), true) else {
        return Err(own);
    };
    let mut feeds = Vec::new();
    for &(a, b) in others {
        if left.is_zero() {
            break;
        }
        let take = b.min(left);
        feeds.push((a, take));
        left -= take;
    }
    if left.is_zero() {
        Ok(Pick::Relay { hub, feeds })
    } else {
        Err(reach)
    }
}

/// Доля, что остаётся после налога перевода между источниками: 1 − max(покупка, продажа, перевод) / 100; None —
/// налог ≥ 100 %. Ошибок нет.
fn hop_keep(taxes: Option<&TokenTaxes>) -> Option<Decimal> {
    let pct = taxes.map_or(Decimal::ZERO, |t| {
        t.buy_pct.max(t.sell_pct).max(t.transfer_pct)
    });
    let keep = Decimal::ONE - pct / Decimal::ONE_HUNDRED;
    (keep > Decimal::ZERO).then_some(keep)
}

/// v × keep вниз (up = false: сколько дойдёт) или v / keep вверх (up = true: сколько отправить). None — keep не
/// задан (налог ≥ 100 %) или числа не переводятся. Ошибок нет.
fn scale(v: U256, keep: Option<Decimal>, up: bool) -> Option<U256> {
    let k = keep?;
    if k == Decimal::ONE {
        return Some(v);
    }
    let d = num::to_decimal(v, 0)?;
    let r = if up { (d / k).ceil() } else { (d * k).floor() };
    num::to_units(r, 0)
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
