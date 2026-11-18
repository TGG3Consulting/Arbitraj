//! Файл: core/credit_alert.rs
//! Что делает: решение 413 (вынесено из credit_apply.rs, ревью t198: файл > 200 строк) — уведомление deal.credit_hold
//! (биржа удержала), доля 0 (алерт человеку, сделка не закрывается), баланс биржи не прочитан (повторное ревью, Б-3:
//! режим застрявшей монеты один раз, потом алерт; оценки без факта нет).
//! Кто вызывает: core/credit_apply.rs (notice, zero), core/remain_fact.rs (unread).
//! От чего зависит: credit_fact.rs, deal_words.rs, manual.rs, sell_next.rs / sell_loop.rs (stuck_as_is), stuck_reason.rs,
//! notify/human.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::storage::journal::Level;

use super::deal::Deal;
use super::deal_words as w;
use super::engine::Core;
use super::remain_others::Others;

/// Уведомление deal.credit_hold, если биржа удержала (доля меньше записи). Принимает: сделку, запись, долю.
/// Возвращает: текст или None — удержания нет. Ошибок нет.
pub fn notice(deal: &Deal, record: Decimal, share: Decimal) -> Option<String> {
    let held = record - share;
    if held <= Decimal::ZERO || record <= Decimal::ZERO {
        return None;
    }
    let t = &deal.direction.asset.ticker;
    let msg = crate::notify::human::Msg::new(
        crate::notify::human::WARN,
        "Биржа зачислила меньше записи депозита",
    )
    .line(format!(
        "{t} на {}: запись {}, на счёте {}",
        crate::notify::human::venue(&deal.at.0),
        w::coins(record, t),
        w::coins(share, t)
    ))
    .kv(
        "Удержано",
        format!(
            "{} ({})",
            w::coins(held, t),
            crate::notify::human::pct(held / record * Decimal::ONE_HUNDRED)
        ),
    )
    .kv("Продаём", w::coins(share, t))
    .id(&deal.id)
    .render();
    Some(msg)
}

/// Доля 0: алерт человеку, сделка не закрывается и ничего не продаёт. Ошибок нет. Возвращает: false.
pub async fn zero(core: &Core, deal: &Deal, o: &Others, text: String) -> bool {
    let t = format!(
        "Монет сделки на счёте не видно: {text}. Бот ничего не продаёт и сделку не закрывает: запись депозита есть, \
         а на счёте для неё ничего не осталось. Что сделать: проверьте баланс {} на {} и историю депозитов биржи; \
         монеты есть — «Продать сейчас» по сделке {} на экране «Ручное управление», нет — «Продано вручную»",
        deal.direction.asset.ticker,
        w::place(core, &deal.at, deal.dex_account.as_ref()),
        deal.id
    );
    core.inner
        .deps
        .log(Level::Warn, "core.credit_fact", Some(&deal.at.0), &t, None)
        .await;
    let mut holders: Vec<String> = o.deals.iter().map(|(id, _)| id.clone()).collect();
    holders.extend(o.pending.iter().map(|p| p.id.clone()));
    holders.extend(o.unclear.iter().cloned());
    holders.sort();
    holders.dedup();
    Box::pin(super::manual::raise_alert_with(
        core,
        deal.clone(),
        None,
        t,
        None,
        holders,
    ))
    .await;
    false
}

/// Что делает: повторное ревью t198 (Б-3) — факт счёта биржи не прочитан (после повторов по правилам шлюза): не
/// продаём и не выводим (не вслепую — ни запись, ни оценка). Первый раз — режим застрявшей монеты без сверки
/// (stuck::handle_as_is: «Ожидание», «Порог убытка», «Перегон» — каждый снова прочитает факт перед продажей или
/// выводом, повтор позже по его механизму); повторно подряд — алерт человеку с причиной (решение 100: кнопки
/// «Продать сейчас», «Беру на себя», «Продано вручную»). Принимает: ядро, сделку, причину. Возвращает: false (вызывающий
/// дальше не ведёт). Ошибок нет.
pub async fn unread(core: &Core, deal: &mut Deal, why: &str) -> bool {
    let t = &deal.direction.asset.ticker;
    let text = format!(
        "{}: баланс {} на {} не прочитан ({why}) — сколько монет у сделки по факту, неизвестно; бот не продаёт и не \
         выводит вслепую (учёт {}{}) (решение 413)",
        w::deal(core, deal),
        t,
        w::place(core, &deal.at, deal.dex_account.as_ref()),
        w::coins(deal.qty, t),
        super::credit_fact::here(deal)
            .map(|c| format!(", запись депозита {}", w::coins(c.record, t)))
            .unwrap_or_default()
    );
    core.inner
        .deps
        .log(
            Level::Warn,
            "core.credit_fact",
            Some(&deal.at.0),
            &text,
            None,
        )
        .await;
    if !deal.fact_unread {
        deal.fact_unread = true;
        let why = super::stuck_reason::StuckReason::NoFact(format!(
            "{text}; режим застрявшей монеты — повтор позже"
        ));
        let job = super::sell_next::stuck_as_is(core, deal.clone(), why);
        super::sell_loop::hop(job).await;
        return false;
    }
    let reason = format!("{text}; повторно — решает человек");
    Box::pin(super::manual::raise_alert(
        core,
        deal.clone(),
        None,
        reason,
        None,
    ))
    .await;
    false
}
