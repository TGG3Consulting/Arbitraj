//! Файл: core/usdt_return.rs
//! Что делает: возврат USDT между биржей и кошельком (решение 34). Режим usdt_return.mode:
//! «в каждой сделке» (per_trade) — после закрытия сделки выручка переводится с B обратно на A
//! (стоимость перегона уже была в расчёте сделки, costs.rs); «отдельно» (separate) — при сверке
//! балансов: на площадке USDT меньше usdt_return.low_balance_usd — перегнать usdt_return.amount_usd
//! с площадки, где USDT хватает (решение 118: донор — только если у него после отдачи не меньше его порога; свободное
//! ведётся по ходу круга). «В каждой сделке» уходит выручка минус комиссия продажи в котируемой монете. Каждый перегон — уведомление usdt.relayed. Сухой режим (решение 74, dry.rs) — «отдельно»
//! не перегоняет; «в каждой сделке» — часть начатой сделки, она доводится как обычно.
//! Кнопка «Продать и перегнать» (решение 101, deal.return_usdt) — перегон выручки этой сделки при любом режиме.
//! Кто вызывает: sell.rs (after_deal), reconcile.rs (separate).
//! От чего зависит: transfers.rs, engine.rs (адрес USDT), dry.rs, settings.
//! Настройки: usdt_return.mode, usdt_return.low_balance_usd, usdt_return.amount_usd (по площадке), venue.quote_asset.

use std::collections::HashMap;

use rust_decimal::Decimal;

use crate::core::types::{Asset, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::{Core, rd};

/// USDT в BSC как актив (тикер — котируемая монета площадки, адрес — из справочника бирж).
fn usdt(core: &Core, venue: &VenueId) -> Result<Asset, String> {
    let ticker = core
        .inner
        .deps
        .settings
        .text(
            "venue.quote_asset",
            At {
                venue: Some(&venue.0),
                coin: None,
            },
        )
        .map_err(|e| e.message)?;
    let addr = rd(&core.inner.usdt).clone()?;
    Ok(Asset {
        ticker,
        bsc_contract: Some(addr),
    })
}

/// Перегон USDT с from на to. Ошибок нет: итог — в журнал и уведомления.
async fn relay(core: &Core, id: &str, from: &VenueId, to: &VenueId, amount: Decimal) {
    let deps = &core.inner.deps;
    let res = match usdt(core, from) {
        // Решение 108: ожидание баланса у отправителя и повтор при откате (usdt_relay_retry.rs).
        Ok(a) => super::usdt_relay_retry::send(core, id, from, to, &a, amount).await,
        Err(e) => Err(e.into()),
    };
    match res {
        Ok((_, t)) => {
            let text = format!(
                "USDT {amount}: {} → {}, зачислено {}",
                from.0, to.0, t.amount
            );
            deps.log(Level::Info, "core.usdt_return", Some(&from.0), &text, None)
                .await;
            deps.notify("usdt.relayed", Some(&from.0), &text).await;
        }
        Err(e) => {
            let text = format!(
                "Перегон USDT {amount}: {} → {} не состоялся: {e}. Деньги остались на {} — перегнать вручную",
                from.0, to.0, from.0
            );
            deps.log(Level::Error, "core.usdt_return", Some(&from.0), &text, None)
                .await;
            // Решение 108: раньше — только журнал; деньги молча оставались не на той площадке.
            deps.notify("usdt.relay_failed", Some(&from.0), &text).await;
        }
    }
}

/// Режим «в каждой сделке» или кнопка «Продать и перегнать» (решение 101): выручка закрытой сделки — обратно на
/// площадку покупки. Ошибок нет.
pub async fn after_deal(core: &Core, deal: &Deal) {
    let mode = core
        .inner
        .deps
        .settings
        .text("usdt_return.mode", At::default());
    // Варианты настройки — per_trade / separate (settings/catalog/trade.rs). Решение 101: кнопка «Продать и
    // перегнать» (deal.return_usdt) — перегон выручки при любом режиме.
    let wanted = deal.return_usdt || mode.ok().as_deref() == Some("per_trade");
    if !wanted {
        return;
    }
    if deal.at == deal.direction.buy_on {
        // Решение 108: продано там же, где куплено (монета не уезжала) — выручка уже на площадке покупки.
        if deal.return_usdt {
            core.inner
                .deps
                .log(
                    Level::Info,
                    "core.usdt_return",
                    Some(&deal.at.0),
                    format!(
                        "Сделка {}: перегон не нужен — продано на {}, выручка уже на площадке покупки",
                        deal.id, deal.at.0
                    ),
                    None,
                )
                .await;
        }
        return;
    }
    // Решение 118 (Я-22): уходит то, что пришло: выручка минус комиссия продажи в котируемой монете площадки.
    let amount = match usdt(core, &deal.at) {
        Ok(q) => deal.proceeds - sell_fee(deal, &q.ticker),
        Err(_) => deal.proceeds,
    };
    let (c, id, from, to) = (
        core.clone(),
        format!("{}u", deal.id),
        deal.at.clone(),
        deal.direction.buy_on.clone(),
    );
    tokio::spawn(async move { relay(&c, &id, &from, &to, amount).await });
}

/// Комиссия продаж сделки в котируемой монете (её нет в выручке). Принимает: сделку, тикер котируемой. Ошибок нет.
fn sell_fee(deal: &Deal, quote: &str) -> Decimal {
    deal.sells
        .iter()
        .filter(|r| r.fee_asset == quote)
        .map(|r| r.fee)
        .sum()
}

/// Режим «отдельно»: по свободным USDT площадок (из сверки). Принимает: площадка → свободно USDT.
/// Ошибок нет: не заданы пороги площадки — она пропускается (журнал пишет reconcile.rs); сухой режим — ничего.
pub async fn separate(core: &Core, free: &HashMap<VenueId, Decimal>) {
    let s = &core.inner.deps.settings;
    if s.text("usdt_return.mode", At::default()).ok().as_deref() != Some("separate") {
        return;
    }
    // Сухой режим (решение 74): перегон без сделки — тоже перевод денег, не выполняется.
    if let Some(why) = super::dry::blocks(s) {
        tracing::info!("перегон USDT «отдельно» пропущен: {why}");
        return;
    }
    // Решение 118 (Я-13): свободное ведётся по ходу круга — донор не отдаёт дважды, получатель не становится донором
    // тех же денег; донор — только если после отдачи у него не меньше его порога (иначе перегон туда-обратно).
    let mut free = free.clone();
    let low_of = |v: &VenueId| s.decimal("usdt_return.low_balance_usd", at(v)).ok();
    let venues: Vec<VenueId> = free.keys().cloned().collect();
    for low_v in &venues {
        let (Some(low), Ok(amount)) = (
            low_of(low_v),
            s.decimal("usdt_return.amount_usd", at(low_v)),
        ) else {
            continue;
        };
        if free.get(low_v).is_none_or(|h| *h >= low) {
            continue;
        }
        let donor = free
            .iter()
            .filter(|(v, f)| *v != low_v && low_of(v).is_some_and(|l| **f - amount >= l))
            .max_by_key(|(_, f)| **f)
            .map(|(v, _)| v.clone());
        if let Some(from) = donor {
            let id = format!("u{}", chrono::Utc::now().format("%Y%m%d%H%M%S%3f"));
            relay(core, &id, &from, low_v, amount).await;
            *free.entry(from).or_default() -= amount;
            *free.entry(low_v.clone()).or_default() += amount;
        }
    }
}

/// Область настройки «площадка». Принимает: площадку. Возвращает: At. Ошибок нет.
fn at(v: &VenueId) -> At<'_> {
    At {
        venue: Some(&v.0),
        coin: None,
    }
}
