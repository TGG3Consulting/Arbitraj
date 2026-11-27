//! A public observation can be warmed without delaying a competing DEX. Only a priced final request reserves inventory.
use super::{
    Query, enabled, firm,
    price::{self, Pairs},
};
use crate::{
    core::types::{Asset, Cost, CostKind, DexRoute, Quote, RouteMode, Side},
    gateway::{quote_stage, source_quote::FirmTerms},
    venues::bsc_dex::{ctx::Ctx, gas_ticker, hashflow, num, parts::Parts, quote::direct, wallet},
};
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{OnceCell, watch},
    time::Instant,
};
type Identity = (u64, u64, u64);
type PairKey = (Identity, Address, Address);
type Prices = HashMap<(PairKey, bool, Decimal), (Instant, watch::Receiver<Option<Option<Model>>>)>;
type Catalog = (Identity, Instant, Arc<OnceCell<Option<Arc<Pairs>>>>);
type Directions = HashMap<(PairKey, Address), Instant>;
#[derive(Clone, Copy)]
pub(super) struct Model {
    order: Query,
    spent: U256,
    net: U256,
    decs: (u8, u8),
}
#[derive(Default)]
struct State {
    catalog: Option<Catalog>,
    prices: Prices,
    reserved: Directions,
}
#[derive(Clone, Default)]
pub(super) struct Feed(Arc<Mutex<State>>);
fn identity(c: &Ctx) -> Option<Identity> {
    Some((
        c.chain_id()?,
        c.settings.generation(),
        crate::secrets::vault::generation(),
    ))
}
pub(super) fn ready(c: &Ctx) -> bool {
    enabled(c) && c.vault.has_secret("dexalot.api_key") && super::profile::Config::read(c).is_ok()
}
impl Feed {
    pub(super) fn hold_signed(
        &self,
        c: &Ctx,
        pair: (Address, Address),
        owner: Address,
        expiry: u64,
    ) {
        if let Some(id) = identity(c) {
            self.hold(((id, pair.0, pair.1), owner), expiry)
        }
    }
    async fn observe(
        &self,
        c: &Ctx,
        pair: (Address, Address),
        buy: bool,
        qty: Decimal,
        wait: bool,
    ) -> Option<Model> {
        let identity = identity(c)?;
        let key = ((identity, pair.0, pair.1), buy, qty);
        let ttl = c.timeout().ok()?;
        let now = Instant::now();
        let mut rx = {
            let mut book = self.0.lock().ok()?;
            book.prices.retain(|_, (end, _)| *end > now);
            let cell = match &book.catalog {
                Some((id, end, cell)) if *id == identity && *end > now => cell.clone(),
                _ => {
                    let cell = Arc::new(OnceCell::new());
                    book.catalog = Some((identity, now + ttl, cell.clone()));
                    cell
                }
            };
            if let Some((_, rx)) = book.prices.get(&key) {
                rx.clone()
            } else {
                // Bound concurrently warming public quantities across a venue. A later pass can request any other size.
                if book
                    .prices
                    .values()
                    .filter(|(_, r)| r.borrow().is_none())
                    .count()
                    >= 4
                {
                    return None;
                }
                let (tx, rx) = watch::channel(None);
                book.prices.insert(key, (now + ttl + ttl, rx.clone()));
                let ctx = c.clone();
                tokio::spawn(async move {
                    let read = async {
                        let catalog = cell
                            .get_or_init(|| async {
                                Pairs::load(&ctx, identity.0).await.ok().map(Arc::new)
                            })
                            .await
                            .as_ref()?;
                        let row = catalog.find(pair.0, pair.1)?;
                        let di = row.precision(pair.0)?;
                        let dout = row.precision(pair.1)?;
                        let raw = direct::ceil_units(qty, if buy { dout } else { di })?;
                        let q = Query {
                            input: pair.0,
                            output: pair.1,
                            qty: raw,
                            by_target: buy,
                        };
                        let level = price::level(&ctx, catalog, row, q, identity.0).await.ok()?;
                        let sell = q.input == row.base_address;
                        let native_order = Query {
                            input: if sell {
                                level.physical.base_address
                            } else {
                                level.physical.quote_address
                            },
                            output: if sell {
                                level.physical.quote_address
                            } else {
                                level.physical.base_address
                            },
                            ..q
                        };
                        Some(Model {
                            order: native_order,
                            spent: level.spent,
                            net: level.net,
                            decs: (di, dout),
                        })
                    };
                    let outcome = tokio::time::timeout(ttl, read).await.ok().flatten();
                    let _ = tx.send(Some(outcome));
                });
                rx
            }
        };
        if wait && rx.borrow().is_none() {
            tokio::time::timeout(ttl, rx.changed()).await.ok()?.ok()?
        }
        let value = *rx.borrow();
        value.flatten()
    }
    fn acquire(&self, key: (PairKey, Address), now: Instant, period: Duration) -> bool {
        let Ok(mut b) = self.0.lock() else {
            return false;
        };
        b.reserved.retain(|_, end| *end > now);
        if let std::collections::hash_map::Entry::Vacant(entry) = b.reserved.entry(key) {
            entry.insert(now + period);
            true
        } else {
            false
        }
    }
    fn hold(&self, key: (PairKey, Address), expiry: u64) {
        let seconds = expiry.saturating_sub(chrono::Utc::now().timestamp().max(0) as u64);
        if let Some(end) = Instant::now().checked_add(Duration::from_secs(seconds))
            && let Ok(mut b) = self.0.lock()
        {
            b.reserved
                .entry(key)
                .and_modify(|v| *v = (*v).max(end))
                .or_insert(end);
        }
    }
}
pub(super) enum Next {
    Keep,
    Indicative(Box<Quote>),
    Reserved,
}
pub(super) async fn evaluate(
    p: &Parts,
    a: &Asset,
    s: Side,
    qty: Decimal,
    pair: (Address, Address),
    baseline: Option<&Quote>,
) -> Next {
    let c = &p.ctx;
    if !ready(c) {
        return Next::Keep;
    }
    let buy = s == Side::Buy;
    let final_stage = quote_stage::is_final();
    let Some(view) = p
        .dexalot
        .feed
        .observe(c, pair, buy, qty, baseline.is_none())
        .await
    else {
        return Next::Keep;
    };
    let (debit, credit) = view.decs;
    let Some(cash) = num::to_decimal(
        if buy { view.spent } else { view.net },
        if buy { debit } else { credit },
    ) else {
        return Next::Keep;
    };
    let Some(rate) = cash.checked_div(qty) else {
        return Next::Keep;
    };
    let (Ok(gas), Ok(ticker)) = (c.dec("dexalot.gas_budget_native"), gas_ticker::read(c)) else {
        return Next::Keep;
    };
    let candidate = Quote {
        venue: c.venue(),
        asset: a.clone(),
        side: s,
        qty,
        fillable_qty: qty,
        avg_price: rate,
        costs: vec![Cost::new(CostKind::Gas, gas, ticker, false)],
        route: Some(DexRoute {
            mode: RouteMode::RfqIndicative,
            legs: vec![],
            block: 0,
            gas_units: 0,
            executable: false,
            exec_note: Some(super::LABEL.into()),
        }),
        signal_route: None,
        at: chrono::Utc::now(),
        pick: Some(super::LABEL.into()),
    };
    if let Some(old) = baseline.filter(|b| b.fillable_qty == qty) {
        let (Some(ours), Some(theirs)) = tokio::join!(
            hashflow::routing::total(p, &candidate),
            hashflow::routing::total(p, old)
        ) else {
            return Next::Keep;
        };
        if if buy { ours >= theirs } else { ours <= theirs } {
            return Next::Keep;
        }
        if !final_stage && old.route.as_ref().is_some_and(|r| r.executable) {
            return Next::Keep;
        }
    }
    let max = if buy {
        match quote_stage::buy_budget(c.place.id, pair.0) {
            Some(v) => {
                let Some(raw) = num::to_units(v, debit) else {
                    return Next::Keep;
                };
                raw
            }
            None => view.spent,
        }
    } else {
        view.order.qty
    };
    if view.spent > max {
        return Next::Keep;
    }
    if !final_stage {
        return Next::Indicative(Box::new(candidate));
    }
    let (Ok(who), Ok(wallet), Some(id), Ok(wait), Ok(ms)) = (
        wallet::signer::address(c),
        c.bound(),
        identity(c),
        c.timeout(),
        c.int("dexalot.min_validity_ms"),
    ) else {
        return Next::Keep;
    };
    let key = ((id, pair.0, pair.1), who);
    if !p
        .dexalot
        .feed
        .acquire(key, Instant::now(), wait.max(Duration::from_millis(ms)))
    {
        return Next::Keep;
    }
    let terms = FirmTerms {
        wallet: wallet.to_string(),
        minimum_raw: if buy { view.order.qty } else { U256::ONE }.to_string(),
        maximum_raw: buy.then(|| max.to_string()),
        allow_trade: true,
        fee_tenths_bps: None,
    };
    let result = tokio::time::timeout(
        wait,
        Box::pin(firm::report(
            p,
            view.order,
            terms,
            Some(who),
            view.decs,
            Some(view.spent),
        )),
    )
    .await;
    if let Ok(Ok(r)) = result
        && let Some(expiry) = r.expires
    {
        p.dexalot.feed.hold(key, expiry);
        Next::Reserved
    } else {
        Next::Keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_one_outstanding_reservation_per_wallet_and_physical_direction() {
        let feed = Feed::default();
        let now = Instant::now();
        let ttl = Duration::from_secs(2);
        let a = Address::repeat_byte(1);
        let b = Address::repeat_byte(2);
        let w = Address::repeat_byte(3);
        let id = (8453, 12, 42);
        let first = ((id, a, b), w);
        assert!(feed.acquire(first, now, ttl));
        assert!(!feed.acquire(first, now + Duration::from_millis(1500), ttl));
        assert!(feed.acquire(((id, b, a), w), now, ttl));
        assert!(feed.acquire(first, now + ttl, ttl));
    }
}
