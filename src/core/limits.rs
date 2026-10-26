//! Файл: core/limits.rs
//! Что делает: сколько денег можно вложить в сделку прямо сейчас (шаг 4 ТЗ, «режем объём по ограничениям»):
//! лимит на сделку (money.max_per_trade_usd, можно по площадке A), на монету с учётом уже открытых
//! сделок (money.max_per_coin_usd), на все переводы в пути (money.max_in_transit_usd) и свободный
//! баланс котируемой монеты на A (балансы — через шлюз). Сухой режим (решение 74, dry.rs): вместо свободного баланса A —
//! виртуальный бюджет dry.budget_usdt, к площадке за балансом не ходим; лимиты money.* — как обычно. Режим не читается
//! здесь, а передаётся вызывающим (зафиксирован на расчёт; у живой сделки — всегда Live).
//! Кто вызывает: core/profit.rs; executor.rs ведёт учёт Exposure.
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

/// Сколько USDT можно вложить в сделку направления сейчас.
/// Возвращает: сумму USDT (> 0). Ошибки: не задан лимит, лимит исчерпан, нет баланса на A; в сухом режиме — не задан
/// dry.budget_usdt.
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
    let (free, source) = available(core, &d.buy_on, &cx.quote[0], mode).await?;
    let cap = per_trade.min(coin_left).min(transit_left).min(free);
    if cap <= Decimal::ZERO {
        return Err(format!(
            "нет бюджета: на сделку {per_trade}, остаток на монету {coin_left}, \
             остаток лимита в пути {transit_left}, {source} {} на {}: {free}",
            cx.quote[0], d.buy_on.0
        ));
    }
    Ok(cap)
}

/// Что делает: сколько денег на площадке A для бюджета — в режиме, ЗАФИКСИРОВАННОМ вызывающим (решение 74): Live —
/// свободный баланс котируемой монеты через шлюз (настройка dry.* здесь не читается: у живой сделки виртуального
/// бюджета не бывает); Dry — виртуальный бюджет. Принимает: ядро, площадку A, котируемую монету, режим.
/// Возвращает: (сумма, откуда). Ошибки: площадка не подключена, сбой балансов, сухой режим без бюджета.
pub async fn available(
    core: &Core,
    venue: &VenueId,
    quote: &str,
    mode: &super::dry::Mode,
) -> Result<(Decimal, &'static str), String> {
    if let super::dry::Mode::Dry(b) = mode {
        return Ok((b.clone()?, "виртуальный бюджет dry.budget_usdt"));
    }
    let a = core.inner.deps.gw.get(venue).await.map_err(|e| e.message)?;
    let balances = a
        .balances()
        .await
        .map_err(|e| format!("{}: балансы: {}", venue.0, e.message))?;
    let free = balances
        .iter()
        .find(|b| b.asset == quote)
        .map(|b| b.free)
        .unwrap_or_default();
    Ok((free, "свободно"))
}
