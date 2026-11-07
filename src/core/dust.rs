//! Файл: core/dust.rs
//! Что делает: пыль после продажи (решение 66): остаток меньше шага количества пары биржи, меньше minQty или по цене
//! продажи меньше minNotional — такой ордер биржа не примет, значит это не «застрявшая монета», а пыль: сделка
//! закрывается, остаток пишется в сделку отдельной строкой (Deal.dust) и остаётся на балансе площадки.
//! У DEX правил пары нет — остаток пылью не считается (продаётся как обычно).
//! Пыль не меньше шага (меньше minQty или minNotional — например после частичного исполнения) — событие deal.dust
//! в журнал и Telegram со стоимостью. Сбор пыли не делается — вопрос Тиграну (решение 66).
//! Кто вызывает: core/sell.rs.
//! От чего зависит: engine.rs, market_read.rs (правила пары из SpotPairs).
//! Настройки: нет (пределы — из справочника пар биржи).

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::gateway::events::SpotPairState;

use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::{Core, rd};

/// Пыль ли остаток. Принимает: ядро, площадку, тикер, остаток, цену продажи. Возвращает: true — продать нельзя.
/// Ошибок нет: правил пары нет — не пыль.
pub fn is_dust(core: &Core, v: &VenueId, ticker: &str, qty: Decimal, price: Decimal) -> bool {
    let m = rd(&core.inner.market);
    m.pair(v, ticker)
        .is_some_and(|p| below_order(p, qty, price))
}

/// Пыль не меньше шага количества пары (не продана из-за minQty / minNotional) — о ней уведомление deal.dust.
/// Ошибок нет: правил пары нет — false.
pub fn above_step(core: &Core, v: &VenueId, ticker: &str, qty: Decimal) -> bool {
    rd(&core.inner.market)
        .pair(v, ticker)
        .is_some_and(|p| qty >= p.step_size)
}

/// Остаток не составляет ордера пары: меньше шага, minQty или minNotional по цене. Ошибок нет.
pub fn below_order(p: &SpotPairState, qty: Decimal, price: Decimal) -> bool {
    qty < p.step_size
        || p.min_qty.is_some_and(|x| qty < x)
        || p.min_notional.is_some_and(|x| qty * price < x)
}

/// Остаток сделки после продажи по цене price: пыль — в Deal.dust, количество 0, запись в хронологию; пыль не меньше
/// шага — событие deal.dust (журнал и Telegram) со стоимостью. Не пыль — без изменений. Ошибок нет.
pub async fn settle(core: &Core, deal: &mut Deal, price: Decimal) {
    let (v, t, qty) = (
        deal.at.clone(),
        deal.direction.asset.ticker.clone(),
        deal.qty,
    );
    // Решение 211: пара в справочнике — по тикеру площадки; в тексте — имя монеты ядра.
    let on = super::venue_ticker::ticker_on(core, &deal.direction.asset, &v);
    if qty <= Decimal::ZERO || !is_dust(core, &v, &on, qty, price) {
        return;
    }
    let text = format!(
        "{}: остаток {qty} {t} на {} — пыль (меньше шага или минимума ордера), ≈ {} USDT по цене продажи; остаётся на балансе",
        deal.id,
        v.0,
        (qty * price).round_dp(4)
    );
    deal.note(text.clone());
    deal.dust += qty;
    deal.qty = Decimal::ZERO;
    if above_step(core, &v, &on, qty) {
        let deps = &core.inner.deps;
        deps.log(Level::Info, "deal.dust", Some(&v.0), &text, None)
            .await;
        let tg = crate::notify::human::Msg::new(
            crate::notify::human::INFO,
            "После продажи остался остаток монеты",
        )
        .line(format!(
            "{} {t} на {} ≈ {}",
            crate::notify::human::qty(qty),
            crate::notify::human::venue(&v.0),
            crate::notify::human::usd(qty * price)
        ))
        .line("Меньше минимального ордера — продать нельзя, остаётся на балансе")
        .id(&deal.id)
        .render();
        deps.notify("deal.dust", Some(&v.0), &tg).await;
    }
}
