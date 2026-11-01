//! Файл: core/limits.rs
//! Что делает: сколько денег можно вложить в сделку (шаг 4 ТЗ, «режем объём по ограничениям»): лимит на сделку
//! (money.max_per_trade_usd, можно по площадке A), на монету с учётом уже открытых сделок (money.max_per_coin_usd),
//! на все переводы в пути (money.max_in_transit_usd). Сухой режим (решение 74, dry.rs) — дополнительно виртуальный
//! бюджет dry.budget_usdt.
//! Решение 94: свободный баланс площадки A в РАСЧЁТЕ кандидата больше не читается — расчёт идёт на каждом тике по
//! многим кандидатам, и биржа блокирует частые запросы баланса (BingX, код 100410). Реальный свободный баланс
//! читается ОДИН раз — прямо перед отправкой ордера покупки (free_now, вызывает executor.rs) и ограничивает списание.
//! Кто вызывает: core/profit.rs (budget), core/executor.rs (free_now); executor.rs ведёт учёт Exposure.
//! От чего зависит: engine.rs, deps.rs (шлюз), dry.rs, settings.
//! Настройки: money.max_per_trade_usd, money.max_per_coin_usd, money.max_in_transit_usd, dry.enabled, dry.budget_usdt.

use std::collections::HashMap;

use rust_decimal::Decimal;

use crate::core::types::{Asset, VenueId};
use crate::settings::read::At;

use super::dirctx::DirCtx;
use super::engine::{Core, lk};

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
    let s = &core.inner.deps.settings;
    let d = &cx.dir;
    let g = |name: &str, venue: Option<&str>| {
        s.decimal(name, At { venue, coin: None })
            .map_err(|e| e.message)
    };
    let per_trade = g("money.max_per_trade_usd", Some(&d.buy_on.0))?;
    let per_coin = g("money.max_per_coin_usd", None)?;
    let transit = g("money.max_in_transit_usd", None)?;
    let ex = lk(&core.inner.exposure).clone();
    let coin_left = per_coin - ex.per_coin.get(&d.asset).copied().unwrap_or_default();
    let transit_left = transit - ex.in_transit;
    let mut cap = per_trade.min(coin_left).min(transit_left);
    let mut virt = String::new();
    // Сухой режим: виртуальный бюджет вместо денег площадки (решение 74).
    if let super::dry::Mode::Dry(b) = mode {
        let b = b.clone()?;
        cap = cap.min(b);
        virt = format!(", виртуальный бюджет dry.budget_usdt {b}");
    }
    if cap <= Decimal::ZERO {
        return Err(format!(
            "нет бюджета: на сделку {per_trade}, остаток на монету {coin_left}, \
             остаток лимита в пути {transit_left}{virt}"
        ));
    }
    Ok(cap)
}

/// Предел списания покупки прямо перед ордером (решение 94): меньшее из бюджета плана и свободного баланса котируемой
/// монеты на A, прочитанного сейчас одним запросом. Принимает: ядро, направление, бюджет плана.
/// Возвращает: (предел, свободно, тикер котируемой монеты). Ошибки: не задан venue.quote_asset у A, сбой балансов,
/// свободно 0.
pub async fn buy_cap(
    core: &Core,
    d: &crate::core::types::Direction,
    budget: Decimal,
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
    let free = free_now(core, &d.buy_on, &quote).await?;
    if free <= Decimal::ZERO {
        return Err(format!("на {} свободно 0 {quote}", d.buy_on.0));
    }
    Ok((budget.min(free), free, quote))
}

/// Свободный баланс котируемой монеты на площадке — ОДИН запрос прямо перед ордером покупки (решение 94).
/// Принимает: ядро, площадку, тикер котируемой монеты. Возвращает: свободную сумму (0, если монеты нет).
/// Ошибки: площадка не подключена, сбой запроса балансов (текст площадки, например лимит частоты).
pub async fn free_now(core: &Core, venue: &VenueId, quote: &str) -> Result<Decimal, String> {
    let a = core.inner.deps.gw.get(venue).await.map_err(|e| e.message)?;
    let balances = a
        .balances()
        .await
        .map_err(|e| format!("{}: балансы: {}", venue.0, e.message))?;
    Ok(balances
        .iter()
        .find(|b| b.asset == quote)
        .map(|b| b.free)
        .unwrap_or_default())
}
