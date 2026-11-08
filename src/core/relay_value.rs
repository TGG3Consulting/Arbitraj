//! Файл: core/relay_value.rs
//! Что делает: оценка перегона застрявшей монеты на другую площадку (режим «Перегон», решение 37) — чистая выручка в
//! USDT. Решение 118 (Я-14): у биржи-отправителя комиссия вывода, минимум и максимум вывода — из справочника сетей
//! ядра (без запроса к бирже: у MEXC это ~5.6 МБ config/getall на каждую оценку), налог перевода — transfer_tax.rs;
//! у биржи-получателя — минимальный депозит из справочника (неизвестен — перегона туда нет). У кошелька DEX —
//! стоимость перевода через шлюз (газ и налог считает кошелёк, к бирже запроса нет).
//! Решение 222: приход на биржу-цель — пыль по правилам пары (dust.rs) — выручка 0.
//! Кто вызывает: core/relay.rs.
//! От чего зависит: dirctx_venue.rs (справочник сетей, свежесть), transfers.rs (сеть BSC), transfer_tax.rs,
//! costs_withdraw.rs (точность вывода), trade_ops.rs (выручка продажи), convert.rs.
//! Настройки: venue.quote_asset, venue.networks_stale_s (через dirctx_venue.rs), cex_cex.untaxed_ok (через transfer_tax.rs).

use rust_decimal::Decimal;

use crate::core::types::{Asset, Cost, CostKind, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::convert;
use super::dirctx_venue as venue;
use super::engine::{Core, rd};

/// Затраты вывода qty монеты с биржи `from` по справочнику ядра, с проверкой минимума и максимума вывода.
/// Ошибки: текст — сети нет в справочнике или он устарел, комиссия неизвестна, сумма вне пределов, налог не измерен.
fn cex_costs(
    core: &Core,
    from: &VenueId,
    to: &VenueId,
    a: &Asset,
    net: &str,
    qty: Decimal,
) -> Result<Vec<Cost>, String> {
    let info = venue::network(core, from, a, net).ok_or_else(|| {
        format!(
            "{}: сети {net} у {} нет в справочнике сетей",
            from.0, a.ticker
        )
    })?;
    venue::fresh(core, &info)?;
    let qty = super::costs_withdraw::floor_precision(qty, Some(&info));
    if let Some(min) = info.withdraw_min
        && qty < min
    {
        return Err(format!("вывод {qty} меньше минимума {min}"));
    }
    if let Some(max) = info.withdraw_max
        && !max.is_zero()
        && qty > max
    {
        return Err(format!("вывод {qty} больше максимума {max}"));
    }
    // Решение 199: фиксированная + доля суммы вывода.
    let fee = info.fee_for(qty).ok_or_else(|| {
        format!(
            "{}: комиссия вывода {} в сети {net} неизвестна",
            from.0, a.ticker
        )
    })?;
    let pct = super::transfer_tax::pct(core, to, a)?;
    let tax = (qty - fee).max(Decimal::ZERO) * pct / Decimal::ONE_HUNDRED;
    Ok(vec![
        Cost::new(CostKind::WithdrawFee, fee, a.ticker.clone(), false),
        Cost::new(CostKind::TransferTax, tax, a.ticker.clone(), false),
    ])
}

/// Минимальный депозит биржи `to` по справочнику ядра. У DEX ограничений нет.
/// Ошибки: текст — сети нет в справочнике, минимум неизвестен, приход меньше минимума.
fn deposit_ok(core: &Core, to: &VenueId, a: &Asset, arrive: Decimal) -> Result<(), String> {
    if is_dex(core, to) {
        return Ok(());
    }
    let net = super::transfers::network(core, a, to)?;
    let info = venue::network(core, to, a, &net).ok_or_else(|| {
        format!(
            "{}: сети {net} у {} нет в справочнике сетей",
            to.0, a.ticker
        )
    })?;
    match info.deposit_min {
        None => Err(format!("{}: минимальный депозит неизвестен", to.0)),
        Some(min) if arrive < min => Err(format!(
            "приход {arrive} меньше минимального депозита {min} на {}",
            to.0
        )),
        Some(_) => Ok(()),
    }
}

/// Площадка — DEX. Ошибок нет.
fn is_dex(core: &Core, v: &VenueId) -> bool {
    rd(&core.inner.kinds)
        .iter()
        .any(|(id, k)| id == v && *k == VenueKind::Dex)
}

/// Чистая выручка (USDT) при перегоне qty с `from` на `to` и продаже там. Ошибка — текст причины.
pub async fn value_at(
    core: &Core,
    from: &VenueId,
    to: &VenueId,
    a: &Asset,
    qty: Decimal,
) -> Result<Decimal, String> {
    let net = super::transfers::network(core, a, from)?;
    let costs = if is_dex(core, from) {
        let gw = core.inner.deps.gw.get(from).await.map_err(|e| e.message)?;
        // Решение 211: площадке — её тикер монеты; затраты — обратно в имя ядра.
        let on = super::venue_ticker::on_venue(core, a, from);
        let c = gw
            .withdraw_cost(&on, &net, qty)
            .await
            .map_err(|e| e.message)?;
        super::venue_ticker::costs_back(core, a, from, c)
    } else {
        cex_costs(core, from, to, a, &net, qty)?
    };
    let coin: Decimal = costs
        .iter()
        .filter(|c| c.asset == a.ticker)
        .map(|c| c.amount)
        .sum();
    let arrive = qty - coin;
    deposit_ok(core, to, a, arrive)?;
    let (v, q) = super::trade_ops::sell_value(core, to, a, arrive).await?;
    // Решение 222: приход меньше шага / minQty / minNotional пары цели (dust.rs, решение 66) — продать там нельзя,
    // выручка 0 (остаются только затраты перевода — перегон не окупается).
    let on_to = super::venue_ticker::ticker_on(core, a, to);
    let v = if super::dust::is_dust(core, to, &on_to, arrive, q.avg_price) {
        Decimal::ZERO
    } else {
        v
    };
    let quote = core
        .inner
        .deps
        .settings
        .text(
            "venue.quote_asset",
            At {
                venue: Some(&from.0),
                coin: None,
            },
        )
        .map_err(|e| e.message)?;
    let cex: Vec<VenueId> = rd(&core.inner.kinds)
        .iter()
        .filter(|(_, k)| *k == VenueKind::Cex)
        .map(|(v, _)| v.clone())
        .collect();
    let rates = convert::rates(
        &core.inner.deps,
        &cex,
        vec![quote],
        &a.ticker,
        q.avg_price,
        &costs,
    )
    .await?;
    let mut out = v;
    for c in costs.iter().filter(|c| c.asset != a.ticker) {
        out -= convert::to_usdt(c, &rates)?;
    }
    Ok(out)
}
