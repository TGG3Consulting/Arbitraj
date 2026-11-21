//! Файл: core/manual_outcome.rs
//! Что делает: решение 177 (Тигран, 05.11.2026) — итог продажи, запущенной кнопкой человека («Продать сейчас»,
//! «Продать и перегнать» — у алерта и у остановленной сделки; sell::BUTTON). Команда отвечает сразу «продажа запущена»
//! (настройки «сколько ждать итог ручной команды» нет — продажа DEX ждёт блока, перевод выручки — минуты), а итог
//! приходит отдельным событием CoreEvent::Command {final: true, deal, ok, text}: Telegram правит сообщение записи —
//! «Выполнено: продано X по Y» или «Не продано: причина» (notify/mirror_send.rs), то же — в журнал core.manual_sale.
//! Продажа не кнопкой (автоматика) — ничего: её итог — обычные уведомления сделки.
//! Решение 235: итог продажи кнопкой у закрытой сделки — куда продали, выручка, возврат USDT на площадку покупки
//! (начат / не нужен / не вернётся сам) и итог сделки; у «Продать и перегнать» — ещё площадка, выбранная проходом
//! оценки (manual_relay.rs пишет её в хронологию сделки и журнал).
//! Кто вызывает: core/sell.rs, core/manual_sell.rs, core/manual_relay.rs.
//! От чего зависит: core/view.rs (CoreEvent), notify/human.rs (тексты, решение 164), deps.rs (журнал).
//! Настройки: нет.

use rust_decimal::Decimal;
use serde_json::json;

use crate::core::types::TradeResult;
use crate::notify::human as hu;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::view::CoreEvent;

/// Продано (всё или часть). Решение 235: у закрытой сделки (вызов после ledger::finish) — ещё выручка, возврат USDT
/// на площадку покупки (начат / не нужен / не вернётся сам — return_text) и итог сделки. Принимает: ядро, сделку,
/// причину продажи (sell::BUTTON — итог шлётся), факт, остаток. Ошибок нет.
pub async fn sold(core: &Core, deal: &Deal, why: &str, r: &TradeResult, left: Decimal) {
    let mut text = format!(
        "Выполнено: продано {} {} на {} по {}, выручка {}",
        hu::qty(r.filled_qty),
        deal.direction.asset.ticker,
        hu::venue(&deal.at.0),
        hu::qty(r.avg_price),
        hu::usd(r.filled_qty * r.avg_price)
    );
    if left > Decimal::ZERO {
        text.push_str(&format!(
            ", остаток {} — режим застрявшей монеты",
            hu::qty(left)
        ));
    }
    if let Some(ret) = return_text(deal) {
        text.push_str(&format!("; {ret}"));
    }
    if let Some(res) = deal.result_usd {
        text.push_str(&format!("; итог сделки {}", hu::signed_usd(res)));
        // Решение 236: возврат USDT ещё идёт — его затраты войдут в итог, когда USDT вернутся.
        if super::deal_return_cost::in_progress(deal) {
            text.push_str(" (без затрат возврата USDT — войдут по факту возврата)");
        }
    }
    send(core, deal, why, true, text).await;
}

/// Решение 235: возврат USDT закрытой сделки словами — по частям возврата (usdt_return_parts.rs): начат (сумма, откуда,
/// куда; зачисление придёт уведомлением usdt.relayed), вернулись, ждёт повтора, не вернётся сам (причина); частей нет, а
/// «Продать и перегнать» продала на площадке покупки — «не нужен». Принимает: сделку. Возвращает: None — возврата нет.
/// Ошибок нет.
pub fn return_text(deal: &Deal) -> Option<String> {
    use super::usdt_return_state::UsdtReturn as U;
    let parts: Vec<String> = super::usdt_return_parts::all(deal)
        .into_iter()
        .map(|p| match p {
            U::Pending { amount, from, to } => format!(
                "возврат {} {} начат (зачисление — отдельным сообщением)",
                hu::usd(*amount),
                hu::route(&from.0, &to.0)
            ),
            U::Done {
                credited, from, to, ..
            } => format!(
                "USDT вернулись {}: зачислено {}",
                hu::route(&from.0, &to.0),
                hu::usd(*credited)
            ),
            U::Retrying {
                amount, from, why, ..
            } => format!(
                "возврат {} с {} ждёт повтора: {why}",
                hu::usd(*amount),
                hu::venue(&from.0)
            ),
            U::NotReturned { amount, at, why } => format!(
                "USDT {} на {} сами не вернутся: {why}",
                hu::usd(*amount),
                hu::venue(&at.0)
            ),
            U::Manual {
                amount,
                to,
                credited,
                ..
            } => match credited {
                // Решение 236, дополнение п. 2: зачислено — ответ человека.
                Some(c) => format!(
                    "{} перегнаны на {} вручную, зачислено {}",
                    hu::usd(*amount),
                    hu::venue(&to.0),
                    hu::usd(*c)
                ),
                None => format!(
                    "{} перегнаны на {} вручную (зачисление не указано — затраты возврата неизвестны)",
                    hu::usd(*amount),
                    hu::venue(&to.0)
                ),
            },
        })
        .collect();
    if !parts.is_empty() {
        return Some(parts.join("; "));
    }
    (deal.return_usdt && deal.at == deal.direction.buy_on).then(|| {
        format!(
            "перегон USDT не нужен — выручка уже на площадке покупки {}",
            hu::venue(&deal.at.0)
        )
    })
}

/// Не продано. Принимает: ядро, сделку, причину продажи, причину отказа (текст для человека). Ошибок нет.
pub async fn failed(core: &Core, deal: &Deal, why: &str, reason: &str) {
    send(core, deal, why, false, format!("Не продано: {reason}")).await;
}

/// Событие итога и журнал — только у продажи кнопкой. Ошибок нет.
async fn send(core: &Core, deal: &Deal, why: &str, ok: bool, text: String) {
    if why != super::sell::BUTTON {
        return;
    }
    core.inner.live.send(CoreEvent::Command(json!({
        "final": true, "deal": deal.id, "ok": ok, "text": text,
    })));
    let level = if ok { Level::Info } else { Level::Warn };
    core.inner
        .deps
        .log(
            level,
            "core.manual_sale",
            Some(&deal.at.0),
            format!("{}: {why}: {text}", deal.id),
            None,
        )
        .await;
}
