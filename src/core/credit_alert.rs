//! Файл: core/credit_alert.rs
//! Что делает: решение 413 (вынесено из credit_apply.rs, ревью t198: файл > 200 строк) — уведомление deal.credit_hold
//! (биржа удержала), доля 0 (алерт человеку, сделка не закрывается), баланс биржи не прочитан (повторное ревью, Б-3:
//! режим застрявшей монеты один раз, потом алерт; оценки без факта нет).
//! Кто вызывает: core/credit_apply.rs (notice, zero), core/remain_fact.rs (unread, Unread, wait_ms).
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

/// Что делать, если баланс биржи не прочитан (третье ревью t198).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unread {
    /// Автоматика: режим застрявшей монеты без сверки один раз, повторно — алерт (Б-3).
    Stuck,
    /// Кнопка человека: сразу итог кнопки с настоящей причиной (С-Г).
    Button,
    /// Первый вывод с биржи после покупки: ждать и перечитывать, без продажи (С-Д); интервал не задан — как Stuck.
    Wait,
}

/// Что делает: интервал перечитывания баланса при ожидании первого вывода — venue.status_poll_ms площадки (тот же опрос
/// статусов переводов). Принимает: ядро, биржу. Возвращает: мс или None — не задан. Ошибок нет.
pub fn wait_ms(core: &Core, v: &crate::core::types::VenueId) -> Option<u64> {
    let at = crate::settings::read::At {
        venue: Some(&v.0),
        coin: None,
    };
    core.inner
        .deps
        .settings
        .integer("venue.status_poll_ms", at)
        .ok()
}

/// Что делает: повторное ревью t198 (Б-3) и третье (С-Г) — факт счёта биржи не прочитан (после повторов по правилу
/// «повторить»): не продаём и не выводим (не вслепую — ни запись, ни оценка). Кнопка человека — сразу итог кнопки с
/// причиной «баланс биржи не прочитан» (manual_outcome::failed), сделка ждёт человека (алерт с той же причиной), без
/// режима застрявшей монеты. Автоматика: первый раз — режим застрявшей монеты без сверки (stuck::handle_as_is —
/// каждый его шаг снова прочитает факт перед продажей или выводом), повторно подряд — алерт человеку (решение 100).
/// Принимает: ядро, сделку, причину, поведение. Возвращает: false (вызывающий дальше не ведёт). Ошибок нет.
pub async fn unread(core: &Core, deal: &mut Deal, why: &str, mode: Unread) -> bool {
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
    deal.note(text.clone());
    if mode == Unread::Button {
        deal.fact_unread = true;
        let reason = format!("баланс биржи не прочитан: {text}");
        let d = deal.clone();
        Box::pin(super::manual::raise_alert(
            core,
            d.clone(),
            None,
            reason.clone(),
            None,
        ))
        .await;
        Box::pin(super::manual_outcome::failed(
            core,
            &d,
            super::sell::BUTTON,
            &reason,
        ))
        .await;
        return false;
    }
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
