//! Файл: core/balance_sell_report.rs
//! Что делает: решение 174 — итог кнопки «Продать» панели «Балансы»: текст «Продано X …, получено Y USDT» (тексты —
//! notify/human.rs, решение 164), запись журнала core.balance_sell с тем, кто нажал, событие Telegram balance.sell.
//! Кто вызывает: core/balance_sell.rs.
//! От чего зависит: notify/human.rs, deps.rs (журнал, уведомления), core/types (TradeResult).
//! Настройки: нет (событие balance.sell включается на экране «Уведомления»).

use rust_decimal::Decimal;
use serde_json::json;

use crate::core::types::{TradeResult, VenueId};
use crate::notify::human as hu;
use crate::storage::journal::Level;

use super::engine::Core;

/// Текст итога продажи. Принимает: факт ордера, сколько продавали, котируемую монету, текст перевода (если был).
/// Возвращает: «Продано X T на биржа по P, получено Y USDT» (комиссия не в USDT — отдельно). Ошибок нет.
pub fn sold_text(r: &TradeResult, qty: Decimal, quote: &str, moved: Option<&str>) -> String {
    let fee_in_quote = r.fee_asset == quote;
    let got = r.filled_qty * r.avg_price - if fee_in_quote { r.fee } else { Decimal::ZERO };
    let mut t = format!(
        "Продано {} {} на {} по {}, получено {} {quote}",
        hu::qty(r.filled_qty),
        r.request.asset.ticker,
        r.request.venue.0,
        hu::qty(r.avg_price),
        hu::qty(got)
    );
    if r.filled_qty < qty {
        t.push_str(&format!(
            " (исполнено {} из {})",
            hu::qty(r.filled_qty),
            hu::qty(qty)
        ));
    }
    if !fee_in_quote && !r.fee.is_zero() {
        t.push_str(&format!("; комиссия {} {}", r.fee, r.fee_asset));
    }
    if let Some(m) = moved {
        t.push_str(&format!("; перед продажей: {m}"));
    }
    t
}

/// Итог — в журнал (core.balance_sell, кто нажал) и в Telegram (balance.sell). Ошибок нет.
pub async fn report(
    core: &Core,
    v: &VenueId,
    account: &str,
    ticker: &str,
    who: &str,
    res: &Result<String, String>,
) {
    let (level, text, msg) = match res {
        Ok(t) => (
            Level::Info,
            format!("«Продать» ({who}): {t}"),
            hu::Msg::new(hu::OK, "Продано из панели «Балансы»").line(t.clone()),
        ),
        Err(e) => (
            Level::Warn,
            format!(
                "«Продать» {ticker} ({account}, {}) ({who}) — отказ: {e}",
                v.0
            ),
            hu::Msg::new(hu::STOP, "Продажа из панели «Балансы» не прошла")
                .line(format!("{ticker} на {}", hu::venue(&v.0)))
                .reason(e),
        ),
    };
    let data = json!({ "account": account, "asset": ticker, "who": who, "ok": res.is_ok() });
    core.inner
        .deps
        .log(level, "core.balance_sell", Some(&v.0), &text, Some(data))
        .await;
    let msg = msg.kv("Счёт", account).kv("Кто", who).render();
    core.inner
        .deps
        .notify("balance.sell", Some(&v.0), &msg)
        .await;
}
