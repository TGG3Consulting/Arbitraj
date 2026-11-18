//! Файл: core/credit_text.rs
//! Что делает: решение 413 (t198) — текст подтверждения доли сделки по факту счёта биржи для хронологии, журнала
//! core.credit_fact и алерта (требование Тиграна 18.11: что произошло, какая сделка, запись депозита, факт, удержание
//! в шт и %, доля, сколько продаём, чьи монеты где).
//! Кто вызывает: core/credit_apply.rs.
//! От чего зависит: deal_words.rs, deposit_hold.rs, credit_split.rs, remain_others.rs, notify/human.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use super::credit_split::Split;
use super::deal::Deal;
use super::deal_words as w;
use super::engine::Core;
use super::remain_others::Others;

/// Что делает: текст распределения (требование Тиграна 18.11: цифры и чьи монеты где). Принимает: ядро, сделку, факт
/// счёта, других, распределение, (запись депозита, доля сделки). Возвращает: текст. Ошибок нет.
pub fn describe(
    core: &Core,
    deal: &Deal,
    fact: Decimal,
    o: &Others,
    sp: &Split,
    (record, share): (Decimal, Decimal),
) -> String {
    let t = &deal.direction.asset.ticker;
    let place = w::place(core, &deal.at, deal.dex_account.as_ref());
    let held = (record - share).max(Decimal::ZERO);
    let pct = if record > Decimal::ZERO {
        held / record * Decimal::ONE_HUNDRED
    } else {
        Decimal::ZERO
    };
    let mut s = format!(
        "{}: зачисление на {place} — запись депозита {}, на счёте сейчас {}",
        w::deal(core, deal),
        w::coins(record, t),
        w::coins(fact, t)
    );
    if o.qty > Decimal::ZERO || !o.pending.is_empty() || !o.unclear.is_empty() {
        s.push_str(&format!(
            "; другие сделки на счёте: {} (подтверждено {})",
            o.words.join("; "),
            w::coins(o.qty, t)
        ));
    }
    if sp.outsider > Decimal::ZERO {
        s.push_str(&format!(
            "; не принадлежат сделкам (было на счёте до вывода): {}",
            w::coins(sp.outsider, t)
        ));
    }
    s.push_str(&format!(
        ". Доля сделки по факту: {}; удержано биржей {} ({}); ожидалось удержание {}",
        w::coins(share, t),
        w::coins(held, t),
        crate::notify::human::pct(pct),
        crate::notify::human::pct(super::deposit_hold::expected_pct(
            core,
            &deal.at,
            &deal.direction.asset
        ))
    ));
    // Повторное ревью (мелкое, «план против факта»): удержание в USDT по цене покупки сделки — в итоге сделки оно уже
    // есть (монет продаётся меньше), отдельной статьёй затрат не дублируется.
    if let Some(p) = deal.buy.as_ref().map(|b| b.avg_price)
        && held > Decimal::ZERO
    {
        s.push_str(&format!(
            " ≈ {} по цене покупки (в итоге сделки — через меньшее количество к продаже)",
            crate::notify::human::usd(held * p)
        ));
    }
    if sp.over {
        s.push_str(" — удержано больше ожидаемого");
    }
    // Третье ревью (С-В): не присвоено этой сделке — доля по факту минус отданное.
    let unassigned = (sp.raw_of(&deal.id).unwrap_or(share) - share).max(Decimal::ZERO);
    if unassigned > Decimal::ZERO {
        s.push_str(&format!(
            "; не присвоено {} — эти монеты на счёте по факту были бы долей сделки, но снимка посторонних монет до \
             вывода нет или через счёт идёт сделка в пути: доля не выше записи × (1 − ожидаемое удержание), они \
             остаются на счёте",
            w::coins(unassigned, t)
        ));
    }
    if sp.excess > Decimal::ZERO {
        s.push_str(&format!(
            "; лишнее на счёте сверх записей ({}) никому не присвоено",
            w::coins(sp.excess, t)
        ));
    }
    s.push_str(&format!(
        ". Бот продаёт только долю сделки: {} (решение 413)",
        w::coins(share, t)
    ));
    s
}
