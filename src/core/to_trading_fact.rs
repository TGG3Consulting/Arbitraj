//! Файл: core/to_trading_fact.rs
//! Что делает: решение 413 (t198) — перевод зачисленного на торговый счёт (решение 102, у BingX — фонд → спот) получил
//! отказ «мало средств»: биржа зачислила на счёт меньше записи депозита (удержание при зачислении, как у WKC на MEXC
//! 18.11). Сумма повтора — факт: свободное этой монеты на счетах, с которых площадка перекладывает на торговый
//! (AccountRole::ToTrading), не больше записи. Переложить чужие монеты на торговый безопасно — это не продажа (так же
//! кнопка «Продать», решение 174); продаёт сделка только свою долю (sell_fact.rs). Один повтор; дальше — как раньше.
//! Кто вызывает: core/transfer_credit.rs (to_trading).
//! От чего зависит: venue_ticker.rs, deps.rs (журнал), gateway (account_balances, account_role).
//! Настройки: нет.

use std::sync::Arc;

use rust_decimal::Decimal;

use crate::gateway::contract::{AccountRole, Venue};
use crate::storage::journal::Level;

use super::engine::Core;
use super::transfer_credit::Leg;

/// Что делает: сумма повтора перевода на торговый по факту счёта (шапка). Принимает: ядро, перевод, шлюз получателя,
/// сумму отказа. Возвращает: Some(сумма) — 0 < факт < сумма отказа; None — факта нет, он не меньше или пуст (дальше —
/// как раньше). Ошибок нет: причина — в журнал core.credit_fact.
pub async fn fund(
    core: &Core,
    leg: &Leg<'_>,
    gw: &Arc<dyn Venue>,
    asked: Decimal,
) -> Option<Decimal> {
    let on = super::venue_ticker::ticker_on(core, leg.asset, leg.to);
    let free = match gw.account_balances().await {
        Ok(accs) => accs
            .iter()
            .filter(|a| a.error.is_none() && gw.account_role(&a.account) == AccountRole::ToTrading)
            .flat_map(|a| a.balances.iter())
            .filter(|b| b.asset == on)
            .map(|b| b.free)
            .sum::<Decimal>(),
        Err(e) => {
            log(
                core,
                leg,
                format!("балансы счетов не прочитаны: {}", e.message),
            )
            .await;
            return None;
        }
    };
    if free <= Decimal::ZERO || free >= asked {
        return None;
    }
    let text = format!(
        "{}: перевод на торговый счёт {} {}: «мало средств» — на счёте перевода {}, переводится то, что есть \
         (удержание биржи при зачислении {}; решение 413)",
        leg.deal,
        asked,
        leg.asset.ticker,
        free,
        asked - free
    );
    log(core, leg, text).await;
    Some(free)
}

/// Запись в журнал core.credit_fact. Ошибок нет.
async fn log(core: &Core, leg: &Leg<'_>, text: String) {
    core.inner
        .deps
        .log(Level::Warn, "core.credit_fact", Some(&leg.to.0), text, None)
        .await;
}
