//! Файл: core/usdt_return.rs
//! Что делает: возврат USDT между биржей и кошельком (решение 34). Режим usdt_return.mode:
//! «в каждой сделке» (per_trade) — после закрытия сделки выручка переводится с B обратно на A
//! (стоимость перегона уже была в расчёте сделки, costs.rs); «отдельно» (separate) — при сверке
//! балансов: на площадке USDT меньше usdt_return.low_balance_usd — перегнать usdt_return.amount_usd
//! с площадки, где USDT хватает. Каждый перегон — уведомление usdt.relayed.
//! Кто вызывает: sell.rs (after_deal), reconcile.rs (separate).
//! От чего зависит: transfers.rs, engine.rs (адрес USDT), settings.
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
        Ok(a) => super::transfers::send(core, id, from, to, &a, amount).await,
        Err(e) => Err(e),
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
            deps.log(
                Level::Error,
                "core.usdt_return",
                Some(&from.0),
                format!("Перегон USDT {} → {} не состоялся: {e}", from.0, to.0),
                None,
            )
            .await;
        }
    }
}

/// Режим «в каждой сделке»: выручка закрытой сделки — обратно на площадку покупки. Ошибок нет.
pub async fn after_deal(core: &Core, deal: &Deal) {
    let mode = core
        .inner
        .deps
        .settings
        .text("usdt_return.mode", At::default());
    // Варианты настройки — per_trade / separate (settings/catalog/trade.rs).
    if mode.ok().as_deref() != Some("per_trade") || deal.at == deal.direction.buy_on {
        return;
    }
    let amount = deal.proceeds;
    let (c, id, from, to) = (
        core.clone(),
        format!("{}u", deal.id),
        deal.at.clone(),
        deal.direction.buy_on.clone(),
    );
    tokio::spawn(async move { relay(&c, &id, &from, &to, amount).await });
}

/// Режим «отдельно»: по свободным USDT площадок (из сверки). Принимает: площадка → свободно USDT.
/// Ошибок нет: не заданы пороги площадки — она пропускается (журнал пишет reconcile.rs).
pub async fn separate(core: &Core, free: &HashMap<VenueId, Decimal>) {
    let s = &core.inner.deps.settings;
    if s.text("usdt_return.mode", At::default()).ok().as_deref() != Some("separate") {
        return;
    }
    for (low_v, have) in free {
        let at = At {
            venue: Some(&low_v.0),
            coin: None,
        };
        let (Ok(low), Ok(amount)) = (
            s.decimal("usdt_return.low_balance_usd", at),
            s.decimal("usdt_return.amount_usd", at),
        ) else {
            continue;
        };
        if *have >= low {
            continue;
        }
        let donor = free
            .iter()
            .filter(|(v, f)| *v != low_v && **f >= amount)
            .max_by_key(|(_, f)| **f);
        if let Some((from, _)) = donor {
            let id = format!("u{}", chrono::Utc::now().format("%Y%m%d%H%M%S%3f"));
            relay(core, &id, from, low_v, amount).await;
        }
    }
}
