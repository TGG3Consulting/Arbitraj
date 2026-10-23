//! Файл: core/fills.rs
//! Что делает: перенос факта исполнения в учёт сделки: сколько монет у бота, сколько вложено и получено,
//! сколько потрачено на комиссии. Комиссия в монете уменьшает количество; в USDT или BNB — идёт в затраты
//! (BNB — по живой цене, convert.rs). Также затраты перевода (Transfer.fee).
//! Кто вызывает: executor.rs, stuck.rs, manual.rs.
//! От чего зависит: deal.rs, convert.rs, settings.
//! Настройки: venue.quote_asset.

use rust_decimal::Decimal;

use crate::core::types::{Cost, CostKind, Side, TradeResult, Transfer, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::convert::{self, Rates};
use super::deal::Deal;
use super::engine::{Core, rd};

/// Затрата в USDT по живым ценам. Ошибка — текст (нет курса): затрата остаётся неучтённой в итоге,
/// и это пишется в журнал (итог сделки помечается).
async fn usdt(
    core: &Core,
    venue: &VenueId,
    coin: &str,
    price: Decimal,
    c: &Cost,
) -> Result<Decimal, String> {
    let quote = core
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
    let bnb = if c.asset == convert::GAS_TICKER {
        let cex: Vec<VenueId> = rd(&core.inner.kinds)
            .iter()
            .filter(|(_, k)| *k == VenueKind::Cex)
            .map(|(v, _)| v.clone())
            .collect();
        Some(convert::bnb_price(&core.inner.deps, &cex, c.amount).await?)
    } else {
        None
    };
    convert::to_usdt(
        c,
        &Rates {
            quote: vec![quote],
            coin: coin.to_string(),
            coin_price: price,
            bnb,
        },
    )
}

/// Добавляет затрату: в монете — уменьшает количество; иначе — в spent (USDT).
async fn charge(core: &Core, deal: &mut Deal, venue: &VenueId, price: Decimal, c: Cost) {
    if c.amount.is_zero() {
        return;
    }
    let coin = deal.direction.asset.ticker.clone();
    if c.asset == coin {
        deal.qty -= c.amount;
        return;
    }
    match usdt(core, venue, &coin, price, &c).await {
        Ok(u) => deal.spent += u,
        Err(e) => {
            let t = format!(
                "{}: затрата {} {} не переведена в USDT ({e}) — итог сделки без неё",
                deal.id, c.amount, c.asset
            );
            deal.note(t.clone());
            core.inner
                .deps
                .log(Level::Warn, "core.ledger", Some(&venue.0), t, None)
                .await;
        }
    }
}

/// Факт покупки или продажи → учёт сделки. Ошибок нет.
pub async fn trade(core: &Core, deal: &mut Deal, r: &TradeResult) {
    let v = r.request.venue.clone();
    match r.request.side {
        Side::Buy => {
            deal.qty += r.filled_qty;
            deal.invested += r.filled_qty * r.avg_price;
        }
        Side::Sell => {
            deal.qty -= r.filled_qty;
            deal.proceeds += r.filled_qty * r.avg_price;
        }
    }
    let fee = Cost::new(CostKind::TradeFee, r.fee, r.fee_asset.clone(), false);
    charge(core, deal, &v, r.avg_price, fee).await;
    deal.note(format!(
        "{:?} на {}: {} по {} (комиссия {} {})",
        r.request.side, v.0, r.filled_qty, r.avg_price, r.fee, r.fee_asset
    ));
}

/// Факт перевода → учёт: монеты теперь на получателе в количестве «сколько дошло»; затраты вывода
/// не в монете (газ кошелька) — в spent (в монете они уже в разнице «отправлено − дошло»).
/// Принимает: сделку, вывод (с комиссией), зачисление, получателя, цену монеты.
pub async fn transfer(
    core: &Core,
    deal: &mut Deal,
    sent: &Transfer,
    t: &Transfer,
    to: &VenueId,
    price: Decimal,
) {
    deal.qty = t.amount;
    deal.at = to.clone();
    if let Some(c) = sent.fee.clone()
        && c.asset != deal.direction.asset.ticker
    {
        charge(core, deal, &sent.venue, price, c).await;
    }
    deal.transfers.push(sent.clone());
    deal.transfers.push(t.clone());
    deal.note(format!(
        "Перевод зачислен на {}: {} {}",
        to.0, t.amount, t.asset.ticker
    ));
}
