//! Price request for a held deal. Quotes and reads only; execution claims the live deal separately.
use std::collections::HashMap;

use futures_util::future::join_all;
use rust_decimal::Decimal;

use super::{deal::Deal, engine::Core};
use crate::core::types::{DexAccountId, Side, VenueId};
use crate::gateway::contract::{LocalSellOrder, LocalSellRules};

pub struct Comparison {
    pub here: Result<Decimal, String>,
    pub targets: Vec<(VenueId, Result<Decimal, String>)>,
    pub skipped: Vec<String>,
    pub prior_cash: Decimal,
    pub here_rest_error: Option<String>,
    wallets: HashMap<VenueId, DexAccountId>,
}

impl Comparison {
    pub fn wallet_for(&self, venue: &VenueId) -> Option<&DexAccountId> {
        self.wallets.get(venue)
    }

    pub fn better_target(&self) -> Option<(&VenueId, Decimal)> {
        let here = self.here.as_ref().ok().copied();
        self.targets
            .iter()
            .filter_map(|(to, result)| {
                let value = *result.as_ref().ok()?;
                (value > Decimal::ZERO && here.is_none_or(|h| value > h)).then_some((to, value))
            })
            .max_by(|(a, x), (b, y)| x.cmp(y).then_with(|| b.0.cmp(&a.0)))
    }

    pub fn difference(&self, value: Decimal) -> Option<(Decimal, Option<Decimal>)> {
        let here = *self.here.as_ref().ok()?;
        let delta = value.checked_sub(here)?;
        let percent = if here > Decimal::ZERO {
            delta
                .checked_div(here)
                .and_then(|ratio| ratio.checked_mul(Decimal::ONE_HUNDRED))
        } else {
            None
        };
        Some((delta, percent))
    }
}

/// Compare the net proceeds of this deal's recorded sales plus its remaining tokens.
/// Each venue's own proceeds return together. No other deal's cash subsidizes the quote.
pub async fn compare(core: &Core, deal: &Deal, fresh_here: bool) -> Result<Comparison, String> {
    if deal.qty <= Decimal::ZERO {
        return Err("У сделки нет монет для продажи".into());
    }
    // quote_qty has already projected this deal's share without assigning funds.
    // Execution applies the normal allocation and verifies the amount before any order.
    let (mut targets, skipped) = super::manual_relay_targets::targets(core, deal);
    targets.sort_by(|a, b| a.0.cmp(&b.0));
    let past = previous_cash(core, deal)?;
    let past = &past;
    let quotes = join_all(std::iter::once(&deal.at).chain(targets.iter()).map(
        |venue| async move {
            let wallet = super::manual_price_exec::wallet(core, deal, venue);
            let mut rest_error = None;
            let result = match &wallet {
                Ok(w) => {
                    let book = if fresh_here && *venue == deal.at {
                        super::sale_preview::rest_book(core, deal)
                            .await
                            .unwrap_or_else(|e| {
                                rest_error = Some(e);
                                None
                            })
                    } else {
                        None
                    };
                    scenario(core, deal, venue, w.as_ref(), past, book, true).await
                }
                Err(e) => Err(e.clone()),
            };
            (venue.clone(), wallet.ok().flatten(), result, rest_error)
        },
    ))
    .await;
    let here_rest_error = quotes.first().and_then(|row| row.3.clone());
    let mut wallets = HashMap::new();
    let mut rows = quotes.into_iter().map(|(venue, wallet, result, _)| {
        if result.is_ok()
            && let Some(w) = wallet
        {
            wallets.insert(venue.clone(), w);
        }
        (venue, result)
    });
    let here = rows.next().expect("the current venue is always quoted").1;
    let targets = rows.collect();
    Ok(Comparison {
        here,
        targets,
        skipped,
        here_rest_error,
        prior_cash: past.iter().map(|(_, v)| *v).sum(),
        wallets,
    })
}

pub(super) fn previous_cash(core: &Core, deal: &Deal) -> Result<Vec<(VenueId, Decimal)>, String> {
    super::usdt_return_parts::sell_venues(deal)
        .into_iter()
        .map(|venue| {
            let symbol = super::balance_sell_check::quote_of(core, &venue)?;
            let cash = super::usdt_return_parts::amount_at(deal, &venue, Some(&symbol));
            Ok((venue, cash))
        })
        .collect()
}

pub(super) async fn scenario(
    core: &Core,
    deal: &Deal,
    venue: &VenueId,
    wallet: Option<&DexAccountId>,
    past: &[(VenueId, Decimal)],
    book: Option<super::exec_book::Book>,
    with_return: bool,
) -> Result<Decimal, String> {
    let asset = &deal.direction.asset;
    let (qty, charges) = if *venue == deal.at {
        (deal.qty, vec![])
    } else {
        let (arrive, costs, _) =
            super::relay_value::arrival(core, &deal.at, venue, asset, deal.qty, wallet).await?;
        (arrive, costs)
    };
    let ticker = super::venue_ticker::ticker_on(core, asset, venue);
    let pair = super::dirctx_venue::pair_rules(core, venue, &ticker)?;
    let sell_qty = match &pair {
        Some(p) => {
            qty - qty
                .checked_rem(p.step_size)
                .ok_or("не удалось округлить объём продажи")?
        }
        None => qty,
    };
    if sell_qty <= Decimal::ZERO {
        return Err("остаток меньше шага продажи".into());
    }
    let (adapter, (mut net, quote, book)) = super::trade_ops::sell_quote_source(
        core,
        wallet,
        venue,
        asset,
        sell_qty,
        Some(&deal.id),
        book,
    )
    .await
    .map_err(|e| e.message)?;
    if let Some(book) = book.as_ref() {
        let worst =
            super::worst::worst_of(venue, book, Side::Sell, sell_qty).map_err(|e| e.message)?;
        super::costs_quote::order_limits(&venue.0, pair.as_ref(), sell_qty, worst.price)
            .map_err(|e| e.to_string())?;
        if let LocalSellRules::Rejected(reason) = adapter.local_sell_rules(&|| {
            let best_bid = book.snap.book()?.bids.first()?.price;
            Some(LocalSellOrder {
                asset: super::venue_ticker::on_venue(core, asset, venue),
                qty: sell_qty,
                worst: worst.price,
                best_bid,
            })
        }) {
            return Err(reason);
        }
    }
    for cost in charges
        .iter()
        .filter(|c| !c.in_price && !super::convert::in_coin(c, &asset.ticker))
    {
        net -= super::fills::usdt(core, &deal.at, &asset.ticker, quote.avg_price, cost).await?;
    }
    net += past.iter().map(|(_, cash)| *cash).sum::<Decimal>();
    if with_return {
        let cash = sale_cash(core, venue, sell_qty * quote.avg_price, &quote.costs)?;
        net -= settlement_cost(core, deal, venue, wallet, past, cash).await?;
    }
    Ok(net)
}

/// Actual quote-currency cash available to withdraw. Other fees reduce PnL separately.
pub(super) fn sale_cash(
    core: &Core,
    venue: &VenueId,
    gross: Decimal,
    costs: &[crate::core::types::Cost],
) -> Result<Decimal, String> {
    let symbol = super::balance_sell_check::quote_of(core, venue)?;
    let fee: Decimal = costs.iter()
        .filter(|c| !c.in_price && c.paid_in.is_none() && c.asset == symbol)
        .map(|c| c.amount)
        .sum();
    Ok(gross - fee)
}

pub(super) fn needs_individual_return(core: &Core, deal: &Deal) -> bool {
    deal.group.as_ref().is_some_and(|g| g.left || super::coin_group_left::recorded(core, deal))
        && super::usdt_return_deal::wanted(core, deal)
}

/// Net incremental exit proceeds. Earlier sale proceeds and trading fees are already booked.
/// Apply only the still-unpaid return cost, once for each actual venue and split.
pub(super) async fn individual_value(
    core: &Core,
    deal: &Deal,
    venue: &VenueId,
    net: Decimal,
    cash: Decimal,
) -> Result<Decimal, String> {
    let past = previous_cash(core, deal)?;
    let wallet = super::manual_price_exec::wallet(core, deal, venue)?;
    let cost = settlement_cost(core, deal, venue, wallet.as_ref(), &past, cash).await
        .map_err(|e| format!("стоимость отдельного возврата USDT неизвестна: {e}"))?;
    Ok(net - cost)
}

async fn settlement_cost(
    core: &Core,
    deal: &Deal,
    venue: &VenueId,
    wallet: Option<&DexAccountId>,
    past: &[(VenueId, Decimal)],
    cash: Decimal,
) -> Result<Decimal, String> {
    let mut total = Decimal::ZERO;
    let parts = return_amounts(past, venue, cash);
    for (from, budget) in parts {
        if budget <= Decimal::ZERO || from == deal.direction.buy_on {
            continue;
        }
        if let Some(reason) =
            super::usdt_return_deal::return_block(core, deal, &from, &deal.direction.buy_on)
        {
            return Err(reason);
        }
        let parts = if deal.group.is_some() {
            super::coin_group_flush::return_parts(core, &from, &deal.direction.buy_on, budget).1
        } else {
            vec![(from, budget)]
        };
        for (from, amount) in parts {
            total +=
                super::usdt_return_quote::cost(core, &from, &deal.direction.buy_on, wallet, amount)
                    .await?;
        }
    }
    Ok(total)
}

/// A second fill on the same venue pays one withdrawal fee; fills elsewhere settle separately.
fn return_amounts(
    past: &[(VenueId, Decimal)],
    venue: &VenueId,
    cash: Decimal,
) -> Vec<(VenueId, Decimal)> {
    let mut parts = past.to_vec();
    if let Some((_, prior)) = parts.iter_mut().find(|(v, _)| v == venue) {
        *prior += cash;
    } else {
        parts.push((venue.clone(), cash));
    }
    parts
}

#[cfg(test)]
mod tests;
