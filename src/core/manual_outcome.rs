//! Файл: core/manual_outcome.rs
//! Что делает: решение 177 (Тигран, 05.11.2026) — итог продажи, запущенной кнопкой человека («Продать сейчас»,
//! «Продать и перегнать» — у алерта и у остановленной сделки; sell::BUTTON). Команда отвечает сразу «продажа запущена»
//! (настройки «сколько ждать итог ручной команды» нет — продажа DEX ждёт блока, перевод выручки — минуты), а итог
//! приходит отдельным событием CoreEvent::Command {final: true, deal, ok, text}: Telegram правит сообщение записи —
//! «Выполнено: продано X по Y» или «Не продано: причина» (notify/mirror_send.rs), то же — в журнал core.manual_sale.
//! Продажа не кнопкой (автоматика) — ничего: её итог — обычные уведомления сделки.
//! Кто вызывает: core/sell.rs.
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

/// Продано (всё или часть). Принимает: ядро, сделку, причину продажи (sell::BUTTON — итог шлётся), факт, остаток.
/// Ошибок нет.
pub async fn sold(core: &Core, deal: &Deal, why: &str, r: &TradeResult, left: Decimal) {
    let mut text = format!(
        "Выполнено: продано {} {} на {} по {}",
        hu::qty(r.filled_qty),
        deal.direction.asset.ticker,
        hu::venue(&deal.at.0),
        hu::qty(r.avg_price)
    );
    if left > Decimal::ZERO {
        text.push_str(&format!(
            ", остаток {} — режим застрявшей монеты",
            hu::qty(left)
        ));
    }
    send(core, deal, why, true, text).await;
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
