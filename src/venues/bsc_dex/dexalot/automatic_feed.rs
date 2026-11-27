//! Хранит публичные цены и допустимые резервации по физической паре и кошельку для automatic.rs.
use super::{
    Query, enabled,
    price::{self, Pairs},
};
use crate::venues::bsc_dex::{
    ctx::Ctx,
    quote::{
        direct,
        rfq_feed::{Identity, Pending, PricesAt, identity},
    },
};
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use std::sync::{Arc, Mutex};
use tokio::{sync::OnceCell, time::Instant};

type Catalog = (Identity, Instant, Arc<OnceCell<Option<Arc<Pairs>>>>);
#[derive(Clone, Copy)]
pub(super) struct Model {
    pub(super) order: Query,
    pub(super) spent: U256,
    pub(super) net: U256,
    pub(super) decs: (u8, u8),
}
#[derive(Clone, Default)]
pub(super) struct Feed {
    catalog: Arc<Mutex<Option<Catalog>>>,
    prices: PricesAt<Model>,
    pub(super) pending: Pending,
}
pub(super) fn ready(c: &Ctx) -> bool {
    enabled(c) && c.vault.has_secret("dexalot.api_key") && super::profile::Config::read(c).is_ok()
}
impl Feed {
    pub(super) fn admits(&self, c: &Ctx, coin: Address, cash: Address) -> bool {
        let Some(id) = identity(c) else { return false };
        let Ok(catalog) = self.catalog.lock() else {
            return true;
        };
        let Some((generation, end, levels)) = catalog.as_ref() else {
            return true;
        };
        if *generation != id || *end <= Instant::now() {
            return true;
        }
        levels
            .get()
            .and_then(Option::as_deref)
            .is_none_or(|v| v.find(coin, cash).is_some())
    }

    pub(super) fn hold_signed(
        &self,
        c: &Ctx,
        pair: (Address, Address),
        owner: Address,
        expiry: u64,
    ) {
        if let Some(id) = identity(c) {
            self.pending.hold(((id, pair.0, pair.1), owner), expiry)
        }
    }
    pub(super) async fn observe(
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
        let cell = {
            let mut catalog = self.catalog.lock().ok()?;
            match &*catalog {
                Some((id, end, cell)) if *id == identity && *end > now => cell.clone(),
                _ => {
                    let cell = Arc::new(OnceCell::new());
                    *catalog = Some((identity, now + ttl, cell.clone()));
                    cell
                }
            }
        };
        let ctx = c.clone();
        self.prices
            .observe(key, ttl, wait, async move {
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
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn only_one_outstanding_reservation_per_wallet_and_physical_direction() {
        let feed = Pending::default();
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
