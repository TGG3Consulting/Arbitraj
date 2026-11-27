//! Shared short-lived public observations and reservation limits for on-demand RFQs.
use crate::venues::bsc_dex::ctx::Ctx;
use alloy_primitives::Address;
use rust_decimal::Decimal;
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::watch, time::Instant};
pub(in crate::venues::bsc_dex) type Identity = (u64, u64, u64);
pub(in crate::venues::bsc_dex) type Direction = (Identity, Address, Address);
pub(in crate::venues::bsc_dex) fn identity(c: &Ctx) -> Option<Identity> {
    Some((
        c.chain_id()?,
        c.settings.generation(),
        crate::secrets::vault::generation(),
    ))
}
type Prices<V> = HashMap<(Direction, bool, Decimal), (Instant, watch::Receiver<Option<Option<V>>>)>;
pub(in crate::venues::bsc_dex) struct PricesAt<V>(Arc<Mutex<Prices<V>>>);
impl<V> Default for PricesAt<V> {
    fn default() -> Self {
        Self(Arc::default())
    }
}
impl<V> Clone for PricesAt<V> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<V: Clone + Send + Sync + 'static> PricesAt<V> {
    pub(in crate::venues::bsc_dex) async fn observe<F>(
        &self,
        key: (Direction, bool, Decimal),
        ttl: Duration,
        wait: bool,
        load: F,
    ) -> Option<V>
    where
        F: Future<Output = Option<V>> + Send + 'static,
    {
        let now = Instant::now();
        let mut rx = {
            let mut prices = self.0.lock().ok()?;
            prices.retain(|_, (end, _)| *end > now);
            if let Some((_, rx)) = prices.get(&key) {
                rx.clone()
            } else {
                if prices
                    .values()
                    .filter(|(_, r)| r.borrow().is_none())
                    .count()
                    >= 4
                {
                    return None;
                }
                let (tx, rx) = watch::channel(None);
                prices.insert(key, (now + ttl + ttl, rx.clone()));
                tokio::spawn(async move {
                    let value = tokio::time::timeout(ttl, load).await.ok().flatten();
                    let _ = tx.send(Some(value));
                });
                rx
            }
        };
        if wait && rx.borrow().is_none() {
            tokio::time::timeout(ttl, rx.changed()).await.ok()?.ok()?
        }
        let result = rx.borrow().clone();
        result.flatten()
    }
}
type Reservations = HashMap<(Direction, Address), Instant>;
#[derive(Clone, Default)]
pub(in crate::venues::bsc_dex) struct Pending(Arc<Mutex<Reservations>>);
impl Pending {
    pub(in crate::venues::bsc_dex) fn acquire(
        &self,
        key: (Direction, Address),
        now: Instant,
        period: Duration,
    ) -> bool {
        let Ok(mut b) = self.0.lock() else {
            return false;
        };
        b.retain(|_, end| *end > now);
        if let std::collections::hash_map::Entry::Vacant(entry) = b.entry(key) {
            entry.insert(now + period);
            true
        } else {
            false
        }
    }
    pub(in crate::venues::bsc_dex) fn hold(&self, key: (Direction, Address), expiry: u64) {
        let secs = expiry.saturating_sub(chrono::Utc::now().timestamp().max(0) as u64);
        if let Some(end) = Instant::now().checked_add(Duration::from_secs(secs))
            && let Ok(mut b) = self.0.lock()
        {
            b.entry(key)
                .and_modify(|v| *v = (*v).max(end))
                .or_insert(end);
        }
    }
}
type Waits = Mutex<HashMap<(String, u64), Instant>>;
pub(in crate::venues::bsc_dex) struct Endpoint {
    key: (String, u64),
    wait: Duration,
}
impl Endpoint {
    fn waits() -> &'static Waits {
        static W: std::sync::OnceLock<Waits> = std::sync::OnceLock::new();
        W.get_or_init(Default::default)
    }
    pub(in crate::venues::bsc_dex) fn available(url: &reqwest::Url, c: &Ctx) -> Option<Self> {
        let key = (url.as_str().trim_end_matches('/').to_owned(), c.chain_id()?);
        let mut rows = Self::waits().lock().ok()?;
        let now = Instant::now();
        rows.retain(|_, v| *v > now);
        (!rows.contains_key(&key)).then_some(Self {
            key,
            wait: c.timeout().ok()?,
        })
    }
    pub(in crate::venues::bsc_dex) fn record(self, reply: &crate::gateway::http::Reply) {
        if reply.status.as_u16() != 429
            && (reply.status.as_u16() != 503 || reply.retry_after_s.is_none())
        {
            return;
        }
        let delay = reply
            .retry_after_s
            .map(Duration::from_secs)
            .unwrap_or(self.wait);
        if let Some(end) = Instant::now().checked_add(delay)
            && let Ok(mut rows) = Self::waits().lock()
        {
            rows.entry(self.key)
                .and_modify(|v| *v = (*v).max(end))
                .or_insert(end);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn healthy_alternative_returns_before_public_io_and_shared_reader_reuses_result() {
        let levels = PricesAt::<u8>::default();
        let key = (
            (
                (8453, 2, 1),
                Address::repeat_byte(1),
                Address::repeat_byte(2),
            ),
            true,
            Decimal::ONE,
        );
        let (tx, rx) = tokio::sync::oneshot::channel();
        let ttl = Duration::from_secs(10);
        assert!(
            levels
                .observe(key, ttl, false, async { rx.await.ok() })
                .await
                .is_none()
        );
        tx.send(42).unwrap();
        assert_eq!(
            levels
                .observe(key, ttl, true, async { panic!("duplicate read") })
                .await,
            Some(42)
        );
    }
}
