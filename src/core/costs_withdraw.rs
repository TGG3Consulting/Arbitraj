//! Файл: core/costs_withdraw.rs
//! Что делает: стоимость вывода монеты с A и возврата USDT с B для полного расчёта (costs.rs). У биржи — комиссия
//! вывода из справочника сетей ядра (core.inner.networks — его держит свежим registry_sync.rs; Presence.info у A,
//! DirCtx.usdt_b у B): в расчёте нет ни одного запроса к бирже (решение 66 — подбор объёма считает десятки объёмов,
//! а config/getall BingX подписанный и с лимитом). У DEX — через шлюз (газ перевода из кошелька, налог перевода).
//! Сумма вывода биржи — вниз до точности вывода (withdrawPrecision).
//! Кто вызывает: core/costs.rs.
//! От чего зависит: dirctx.rs, deps.rs (шлюз), size.rs (Fail), core/types.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::{CoinNetworkInfo, Cost, CostKind};

use super::deps::Deps;
use super::dirctx::DirCtx;
use super::size::Fail;

/// Сумма вывода с A вниз до точности вывода биржи (у DEX и без точности — без изменений). Ошибок нет.
pub fn floor_precision(qty: Decimal, info: Option<&CoinNetworkInfo>) -> Decimal {
    match info.and_then(|i| i.withdraw_precision) {
        Some(p) => qty.trunc_with_scale(p),
        None => qty,
    }
}

/// Комиссия вывода биржи из справочника. Принимает: сеть монеты, тикер (для текста). Ошибки: Stop — не сообщена.
fn fee_line(info: &CoinNetworkInfo) -> Result<Cost, Fail> {
    let fee = info.withdraw_fee.ok_or_else(|| {
        Fail::Stop(format!(
            "{}: комиссия вывода {} в сети {} неизвестна",
            info.venue.0, info.asset.ticker, info.network
        ))
    })?;
    Ok(Cost::new(
        CostKind::WithdrawFee,
        fee,
        info.asset.ticker.clone(),
        false,
    ))
}

/// Стоимость вывода монеты с A. Принимает: шлюз, контекст, сумму вывода. Возвращает: затраты.
/// Ошибки: Stop — комиссия неизвестна, сбой шлюза DEX.
pub async fn withdraw_a(deps: &Deps, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
    if let Some(info) = &cx.a.info {
        return Ok(vec![fee_line(info)?]);
    }
    let d = &cx.dir;
    let a = deps.gw.get(&d.buy_on).await.map_err(|e| e.message)?;
    a.withdraw_cost(&d.asset, &cx.net_a, amount)
        .await
        .map_err(|e| Fail::Stop(format!("{}: стоимость вывода: {}", d.buy_on.0, e.message)))
}

/// Стоимость возврата USDT с B. Принимает: шлюз, контекст, сумму. Возвращает: затраты (как их отдаёт площадка).
/// Ошибки: Stop — у биржи B нет сети котируемой монеты в справочнике, комиссия неизвестна, сбой шлюза DEX.
pub async fn return_b(deps: &Deps, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
    let d = &cx.dir;
    if cx.b.info.is_some() {
        let info = cx.usdt_b.as_ref().ok_or_else(|| {
            Fail::Stop(format!(
                "{}: сети {} у {} нет в справочнике — стоимость возврата неизвестна",
                d.sell_on.0, cx.net_b, cx.usdt.ticker
            ))
        })?;
        return Ok(vec![fee_line(info)?]);
    }
    let b = deps.gw.get(&d.sell_on).await.map_err(|e| e.message)?;
    b.withdraw_cost(&cx.usdt, &cx.net_b, amount)
        .await
        .map_err(|e| {
            Fail::Stop(format!(
                "{}: стоимость возврата USDT: {}",
                d.sell_on.0, e.message
            ))
        })
}
