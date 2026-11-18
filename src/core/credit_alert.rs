//! Файл: core/credit_alert.rs
//! Что делает: решение 413 (вынесено из credit_apply.rs, ревью t198: файл > 200 строк) — уведомление deal.credit_hold
//! (биржа удержала), доля 0 (алерт человеку, сделка не закрывается) и оценка доли при непрочитанном балансе.
//! Кто вызывает: core/credit_apply.rs (notice, zero), core/remain_fact.rs (estimate).
//! От чего зависит: credit_fact.rs, credit_lock.rs, deposit_hold.rs, deal_words.rs, manual.rs, notify/human.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::storage::journal::Level;

use super::credit_fact::Basis;
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

/// Что делает: баланс не прочитан, сделка ждёт доли — оценка «запись × (1 − ожидаемое удержание)» (шапка), основание
/// Estimate. Принимает: ядро, сделку, причину. Ошибок нет.
pub async fn estimate(core: &Core, deal: &mut Deal, why: &str) {
    let Some(c) = super::credit_fact::here(deal) else {
        return;
    };
    let record = c.record;
    let pct = super::deposit_hold::expected_pct(core, &deal.at, &deal.direction.asset);
    let q = (record * (Decimal::ONE - pct / Decimal::ONE_HUNDRED)).max(Decimal::ZERO);
    let t = &deal.direction.asset.ticker;
    let text = format!(
        "{}: баланс {} не прочитан ({why}) — зачисление не подтверждено фактом; продаётся оценка: запись депозита {} × \
         (1 − ожидаемое удержание {}) = {}; если биржа не удержала, остаток до {} останется на счёте — проверьте \
         «Балансы» (решение 413)",
        w::deal(core, deal),
        w::place(core, &deal.at, deal.dex_account.as_ref()),
        w::coins(record, t),
        crate::notify::human::pct(pct),
        w::coins(q, t),
        w::coins(record - q, t)
    );
    super::credit_fact::confirm(deal, q, Basis::Estimate);
    deal.note(text.clone());
    super::credit_lock::sync(core, deal);
    core.inner
        .deps
        .log(
            Level::Warn,
            "core.credit_fact",
            Some(&deal.at.0),
            text,
            None,
        )
        .await;
}
