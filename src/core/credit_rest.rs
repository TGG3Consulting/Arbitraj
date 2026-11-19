//! Файл: core/credit_rest.rs
//! Что делает: решение 413 (пятое – седьмое ревью t198) — остаток «не присвоено» (CreditFact.rest) не теряется и не
//! становится фантомом. По решению 76 счёт биржи — только бота: всё, что на нём сверх учёта сделок, — монеты сделок
//! бота. Остаток решается только в ТИХИЙ МОМЕНТ счёта (биржа, монета): нет других ждущих доли, сделок в пути сюда /
//! уходящих отсюда, продаж с неизвестным исходом и сделок с остатком, которые сейчас в работе. Остатки других сделок,
//! что не в работе (остановлены, ждут человека, ждут тихий момент после продажи), — не помеха, а участники
//! СОВМЕСТНОГО решения (седьмое ревью, Б-1): пул = факт − подтверждённые всех прочих (и «Беру на себя»); пул делится
//! между всеми сделками с остатком: сначала их подтверждённые количества (не хватает — поровну по доле количества),
//! затем добавка пропорционально остаткам, каждой не больше «количество + остаток» (share_out). Одно чтение на счёт,
//! под одним замком (credit_rest_lock.rs).
//! Тексты: меньше количества — Warn и Telegram («не хватает»); больше ожидаемого по удержанию — «удержание биржи меньше
//! ожидаемого»; больше записи депозита — «взято сверх зачисления N — остатки прошлых сделок (решение 76)» (Info).
//! Ожидание после продажи, отдельная позиция и круг сверки — credit_rest_park.rs.
//! Кто вызывает: core/credit_rest_lock.rs, tests.
//! От чего зависит: credit_fact.rs, credit_lock.rs (unsure), remain_others.rs, deal_words.rs.
//! Настройки: нет.

use std::collections::HashSet;

use rust_decimal::Decimal;

use super::deal::Deal;
use super::deal_words as w;
use super::engine::Core;
use super::remain_others::Others;

pub use super::credit_rest_lock::{Locked, Res, for_button, locked};

/// Что делает: почему момент счёта не тихий (шапка); None — тихий (остатки других, что не в работе, — не помеха).
/// Принимает: ядро, сделку, учёт других. Ошибок нет.
pub fn busy(core: &Core, deal: &Deal, o: &Others) -> Option<String> {
    let idle: HashSet<&str> = o
        .rests
        .iter()
        .filter(|r| r.3)
        .map(|r| r.0.as_str())
        .collect();
    let unclear: Vec<&String> = o
        .unclear
        .iter()
        .filter(|id| !idle.contains(id.as_str()))
        .collect();
    let unsure = super::credit_lock::unsure(core, &deal.at, &deal.direction.asset, &deal.id);
    if o.pending.is_empty() && unclear.is_empty() && o.leaving.is_empty() && !unsure {
        return None;
    }
    let mut why = o.words.clone();
    if unsure {
        why.push("продажа другой сделки с неизвестным исходом".into());
    }
    Some(why.join("; "))
}

/// Что делает: дележ пула между сделками с остатком (шапка). Принимает: пул, (количество, остаток) каждой, точность.
/// Возвращает: новые количества по порядку. Ошибок нет.
pub fn share_out(pool: Decimal, parts: &[(Decimal, Decimal)], scale: u32) -> Vec<Decimal> {
    let base: Decimal = parts.iter().map(|p| p.0).sum();
    let rests: Decimal = parts.iter().map(|p| p.1).sum();
    if pool <= Decimal::ZERO {
        return vec![Decimal::ZERO; parts.len()];
    }
    if pool <= base {
        return parts
            .iter()
            .map(|p| (p.0 * pool / base).trunc_with_scale(scale))
            .collect();
    }
    let extra = (pool - base).min(rests);
    parts
        .iter()
        .map(|p| {
            let add = if rests.is_zero() {
                Decimal::ZERO
            } else {
                p.1 * extra / rests
            };
            (p.0 + add).min(p.0 + p.1).trunc_with_scale(scale)
        })
        .collect()
}

/// Итог решения остатка одной сделки: тексты для журнала.
#[derive(Default)]
pub struct Resolved {
    /// Вышло меньше количества — Warn и Telegram.
    pub short: Option<String>,
    /// Больше ожидаемого по удержанию или больше записи — Info.
    pub over: Option<String>,
}

/// Что делает: записывает решённое количество сделке (остатка больше нет, отметка «исход неизвестен» снята) и тексты.
/// Принимает: сделку, новое количество, факт, подтверждённые прочих, ожидаемое удержание %. Возвращает: Resolved.
/// Ошибок нет.
pub fn apply_one(
    deal: &mut Deal,
    qty: Decimal,
    fact: Decimal,
    others: Decimal,
    hold: Decimal,
) -> Resolved {
    let Some(rest) = super::credit_fact::rest(deal) else {
        return Resolved::default();
    };
    let t = deal.direction.asset.ticker.clone();
    let before = deal.qty;
    super::credit_fact::confirm(deal, qty, super::credit_fact::Basis::Balance);
    deal.sale_unsure = false;
    let mut text = format!(
        "Остаток зачисления решён в тихий момент счёта (решение 413): было {}, не присвоено {}, на счёте {}, у других \
         {} — у сделки {}",
        w::coins(before, &t),
        w::coins(rest, &t),
        w::coins(fact, &t),
        w::coins(others, &t),
        w::coins(qty, &t)
    );
    let record = super::credit_fact::here(deal).map_or(Decimal::ZERO, |c| c.record);
    let sold: Decimal = deal.sells.iter().map(|s| s.filled_qty).sum();
    let expected = record * (Decimal::ONE - hold / Decimal::ONE_HUNDRED);
    let mut over = None;
    if qty + sold > record {
        let n = w::coins(qty + sold - record, &t);
        text.push_str(&format!(
            "; взято сверх зачисления {n} — остатки прошлых сделок (решение 76)"
        ));
        over = Some(format!("{}: {text}", deal.id));
    } else if qty + sold > expected {
        let n = w::coins(qty + sold - expected, &t);
        text.push_str(&format!(
            "; больше ожидаемого на {n} — удержание биржи меньше ожидаемого"
        ));
        over = Some(format!("{}: {text}", deal.id));
    }
    deal.note(text.clone());
    let short = (qty < before).then(|| {
        format!(
            "{}: {text}; не хватает {}",
            deal.id,
            w::coins(before - qty, &t)
        )
    });
    Resolved { short, over }
}

/// Что делает: решение остатка одной сделки без других с остатком (чистая часть для тестов и совместного решения из
/// одной сделки): min(количество + остаток, факт − подтверждённые других). Принимает: сделку, факт, учёт других,
/// ожидаемое удержание %. Возвращает: Resolved. Ошибок нет.
pub fn resolve(deal: &mut Deal, fact: Decimal, o: &Others, hold: Decimal) -> Resolved {
    let Some(rest) = super::credit_fact::rest(deal) else {
        return Resolved::default();
    };
    let qty = (deal.qty + rest).min((fact - o.qty).max(Decimal::ZERO));
    apply_one(deal, qty, fact, o.qty, hold)
}
