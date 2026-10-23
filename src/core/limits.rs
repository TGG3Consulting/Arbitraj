//! Файл: core/limits.rs
//! Что делает: сколько денег можно вложить в сделку прямо сейчас (шаг 4 ТЗ, «режем объём по ограничениям»):
//! лимит на сделку (money.max_per_trade_usd, можно по площадке A), на монету с учётом уже открытых
//! сделок (money.max_per_coin_usd), на все переводы в пути (money.max_in_transit_usd) и свободный
//! баланс котируемой монеты на A (балансы — через шлюз).
//! Кто вызывает: core/profit.rs; executor.rs ведёт учёт Exposure.
//! От чего зависит: engine.rs, deps.rs (шлюз), settings.
//! Настройки: money.max_per_trade_usd, money.max_per_coin_usd, money.max_in_transit_usd.

use std::collections::HashMap;

use rust_decimal::Decimal;

use crate::core::types::Asset;
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
/// Возвращает: сумму USDT (> 0). Ошибки: не задан лимит, лимит исчерпан, нет баланса на A.
pub async fn budget(core: &Core, cx: &DirCtx) -> Result<Decimal, String> {
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
    let a = core
        .inner
        .deps
        .gw
        .get(&d.buy_on)
        .await
        .map_err(|e| e.message)?;
    let balances = a
        .balances()
        .await
        .map_err(|e| format!("{}: балансы: {}", d.buy_on.0, e.message))?;
    let free = balances
        .iter()
        .find(|b| b.asset == cx.quote[0])
        .map(|b| b.free)
        .unwrap_or_default();
    let cap = per_trade.min(coin_left).min(transit_left).min(free);
    if cap <= Decimal::ZERO {
        return Err(format!(
            "нет бюджета: на сделку {per_trade}, остаток на монету {coin_left}, \
             остаток лимита в пути {transit_left}, свободно {} на {}: {free}",
            cx.quote[0], d.buy_on.0
        ));
    }
    Ok(cap)
}
