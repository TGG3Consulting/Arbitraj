//! Файл: report/snap_price.rs
//! Что делает: решение 257 (приёмка t90, п. 2) — цена монеты для «Слепка по балансам» БЕЗ влияния на торговлю: только
//! Venue::quote площадки (gateway/contract.rs: «котировка на объём по текущим данным» — стакан биржи в памяти адаптера,
//! пулы DEX) и чтение балансов адаптера. НЕ используется путь ордера (core/exec_book.rs: кэш REST-снимков и память
//! неудач exec_book_fail.rs общие с ордерами; core/trade_ops.rs; gateway/patience.rs exec_scope) и память панели
//! «Балансы» (core/balances_cache.rs) — ничего общего с торговлей отчёт не пишет (страж — tests/report_snap.rs).
//! Каждый запрос — до общего срока слепка (report.snapshot_timeout_ms, report/snap.rs).
//! Цена за монету = (количество × средняя цена продажи всего количества − комиссии продажи вне цены в котируемой монете
//! или в самой монете) / количество; комиссия в другой валюте не вычитается — пометка в строке.
//! Кто вызывает: report/snap.rs.
//! От чего зависит: gateway (Venue::quote, balance), core/venue_ticker.rs (тикер площадки), core/types.
//! Настройки: нет своих (venue.quote_asset — читает snap.rs).

use std::sync::Arc;

use rust_decimal::Decimal;
use tokio::time::Instant;

use crate::core::Core;
use crate::core::types::{Asset, Side, VenueId};
use crate::gateway::contract::Venue;

use super::rows::f;

/// Ожидание до срока слепка. Принимает: срок, будущее. Возвращает: результат или «тайм-аут слепка».
pub async fn until<T>(deadline: Instant, fut: impl Future<Output = T>) -> Result<T, String> {
    tokio::time::timeout_at(deadline, fut).await.map_err(|_| {
        "тайм-аут слепка (report.snapshot_timeout_ms) — площадка не ответила вовремя".to_string()
    })
}

/// Цена за монету и пометка (см. шапку). Принимает: ядро, адаптер (уже со счётом DEX), площадку, монету ядра,
/// количество, котируемую монету площадки, срок. Возвращает: (цена, пометка) или причину, почему цены нет.
pub async fn price(
    core: &Core,
    gw: &Arc<dyn Venue>,
    v: &VenueId,
    a: &Asset,
    qty: Decimal,
    quote: &str,
    deadline: Instant,
) -> Result<(f64, String), String> {
    if qty <= Decimal::ZERO {
        return Err("количество не больше 0".into());
    }
    let on = crate::core::venue_ticker::on_venue(core, a, v);
    let q = until(deadline, gw.quote(&on, Side::Sell, qty))
        .await?
        .map_err(|e| e.message)?;
    if q.fillable_qty < qty {
        return Err(format!("глубины хватает на {} из {qty}", q.fillable_qty));
    }
    let mut net = qty * q.avg_price;
    let mut skipped: Vec<String> = Vec::new();
    for c in q.costs.iter().filter(|c| !c.in_price) {
        if c.asset == quote {
            net -= c.amount;
        } else if c.asset == on.ticker {
            net -= c.amount * q.avg_price;
        } else {
            skipped.push(format!("{} {}", c.amount, c.asset));
        }
    }
    let note = if skipped.is_empty() {
        "котировка площадки (только чтение), чистыми от комиссии продажи".to_string()
    } else {
        format!(
            "котировка площадки (только чтение); комиссия продажи {} не вычтена — нет пересчёта в отчёте",
            skipped.join(", ")
        )
    };
    Ok((f(net / qty), note))
}

/// Остаток монеты на площадке (свободно + заморожено) чтением адаптера. Принимает: адаптер, тикер площадки, срок.
/// Возвращает: количество (строки нет — 0) или причину.
pub async fn balance(
    gw: &Arc<dyn Venue>,
    ticker: &str,
    deadline: Instant,
) -> Result<Decimal, String> {
    until(deadline, gw.balance(ticker))
        .await?
        .map(|b| b.map_or(Decimal::ZERO, |x| x.free + x.locked))
        .map_err(|e| e.message)
}
