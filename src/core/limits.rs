//! Файл: core/limits.rs
//! Что делает: сколько денег можно вложить в сделку (шаг 4 ТЗ, «режем объём по ограничениям»): лимит на сделку
//! (money.max_per_trade_usd, можно по площадке A), на монету с учётом уже открытых сделок (money.max_per_coin_usd),
//! на все переводы в пути (money.max_in_transit_usd). Сухой режим (решение 74, dry.rs) — дополнительно виртуальный
//! бюджет dry.budget_usdt.
//! Решение 94: свободный баланс площадки A в РАСЧЁТЕ кандидата больше не читается — расчёт идёт на каждом тике по
//! многим кандидатам, и биржа блокирует частые запросы баланса (BingX, код 100410). Реальный свободный баланс
//! читается ОДИН раз — прямо перед отправкой ордера покупки (free_now, вызывает executor.rs) и ограничивает списание.
//! Решение 141: составляющие бюджета и какая ограничила — limits_parts.rs (budget — меньшее из них).
//! Кто вызывает: core/profit.rs (budget), core/executor.rs (free_now); executor.rs ведёт учёт Exposure.
//! От чего зависит: engine.rs, deps.rs (шлюз), dry.rs, settings.
//! Настройки: money.max_per_trade_usd, money.max_per_coin_usd, money.max_in_transit_usd, dry.enabled, dry.budget_usdt.

use std::collections::HashMap;

use rust_decimal::Decimal;

use crate::core::types::{Asset, VenueId};
use crate::settings::read::At;

use super::dirctx::DirCtx;
use super::engine::Core;

/// Деньги в открытых сделках (ведёт executor.rs): вложено по монете и сумма в пути.
#[derive(Default, Debug, Clone)]
pub struct Exposure {
    pub per_coin: HashMap<Asset, Decimal>,
    pub in_transit: Decimal,
}

/// Сколько USDT можно вложить в сделку направления по лимитам (без запроса баланса площадки — решение 94).
/// Принимает: ядро, контекст направления, режим (зафиксирован вызывающим). Возвращает: сумму USDT (> 0).
/// Ошибки: не задан лимит, лимит исчерпан; в сухом режиме — не задан dry.budget_usdt.
pub async fn budget(core: &Core, cx: &DirCtx, mode: &super::dry::Mode) -> Result<Decimal, String> {
    // Решение 141: составляющие — limits_parts.rs (та же формула; «без потолка» пишет, какая ограничила).
    let p = super::limits_parts::parts(core, &cx.dir, mode)?;
    let cap = p.cap();
    if cap <= Decimal::ZERO {
        let virt = p
            .dry
            .map(|b| format!(", виртуальный бюджет dry.budget_usdt {b}"))
            .unwrap_or_default();
        return Err(format!(
            "нет бюджета: на сделку {}, остаток на монету {}, остаток лимита в пути {}{virt}",
            p.per_trade, p.coin_left, p.transit_left
        ));
    }
    Ok(cap)
}

/// Предел списания покупки прямо перед ордером (решение 94): меньшее из бюджета плана и свободного баланса котируемой
/// монеты на A, прочитанного сейчас одним запросом. Принимает: ядро, направление, бюджет плана, счёт DEX сделки.
/// Возвращает: (предел, свободно, тикер котируемой монеты). Ошибки: не задан venue.quote_asset у A, сбой балансов,
/// свободно 0.
pub async fn buy_cap(
    core: &Core,
    d: &crate::core::types::Direction,
    budget: Decimal,
    acc: Option<&crate::core::types::DexAccountId>,
) -> Result<(Decimal, Decimal, String), String> {
    let at = At {
        venue: Some(&d.buy_on.0),
        coin: None,
    };
    let quote = core
        .inner
        .deps
        .settings
        .text("venue.quote_asset", at)
        .map_err(|e| e.message)?;
    let free = free_now(core, &d.buy_on, acc, &quote).await?;
    if free <= Decimal::ZERO {
        return Err(format!("на {} свободно 0 {quote}", d.buy_on.0));
    }
    Ok((budget.min(free), free, quote))
}

/// Свободный баланс котируемой монеты на площадке — ОДИН запрос прямо перед ордером покупки (решение 94).
/// Принимает: ядро, площадку, счёт DEX (решение 132; None — биржа), тикер котируемой монеты. Возвращает: свободную сумму (0, если монеты нет).
/// Ошибки: площадка не подключена, сбой запроса балансов (текст площадки, например лимит частоты).
pub async fn free_now(
    core: &Core,
    venue: &VenueId,
    acc: Option<&crate::core::types::DexAccountId>,
    quote: &str,
) -> Result<Decimal, String> {
    // Решение 132: у DEX — баланс кошелька счёта (сделки или перегона).
    let a = core
        .inner
        .deps
        .gw
        .get_as(venue, acc)
        .await
        .map_err(|e| e.message)?;
    // Решение 113: баланс одной монеты — у DEX один balanceOf, а не запрос по каждому токену кошелька (~11 с);
    // у биржи — тот же один REST балансов (по умолчанию контракта).
    Ok(a.balance(quote)
        .await
        .map_err(|e| format!("{}: балансы: {}", venue.0, e.message))?
        .map(|b| b.free)
        .unwrap_or_default())
}
