//! Уровни Native orderbook задают порции входного ERC20. Минимум API указан в сырых единицах.
//! https://docs.native.org/native-dev/build-with-native/swap-aggregators/firmquote-swap-apis/get-orderbook
use super::{Profile, wire};
use crate::{
    core::types::Side,
    gateway::{
        events::{MarketEvent, SourceId},
        http,
    },
    venues::bsc_dex::{hashflow::availability, parts::Parts},
};
use alloy_primitives::{Address, B256, U256};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

struct Row {
    minimum: U256,
    levels: Vec<(Decimal, Decimal)>,
}
#[derive(Default)]
pub(super) struct Depth(BTreeMap<(Address, Address), Row>);
impl Depth {
    fn parse(v: &Value) -> Option<Self> {
        let mut rows = BTreeMap::new();
        for item in v.as_array()? {
            if item["side"] != "bid" {
                continue;
            }
            let i = item["base_address"].as_str()?.parse::<Address>().ok()?;
            let o = item["quote_address"].as_str()?.parse::<Address>().ok()?;
            let minimum = wire::quantity(&item["minimum_in_base"])?;
            let d = |v: &Value| {
                let value = match v {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => return None,
                };
                if value.contains(['e', 'E']) {
                    Decimal::from_scientific(&value).ok()
                } else {
                    Decimal::from_str_exact(&value).ok()
                }
            };
            let levels = item["levels"]
                .as_array()?
                .iter()
                .map(|v| {
                    let [qty, price] = v.as_array()?.as_slice() else {
                        return None;
                    };
                    let (q, p) = (d(qty)?, d(price)?);
                    (q > Decimal::ZERO && p > Decimal::ZERO).then_some((q, p))
                })
                .collect::<Option<Vec<_>>>()?;
            if levels.is_empty()
                || i == o
                || i.is_zero()
                || o.is_zero()
                || rows.insert((i, o), Row { minimum, levels }).is_some()
            {
                return None;
            }
        }
        Some(Self(rows))
    }
    pub(super) fn value(
        &self,
        input: Address,
        output: Address,
        side: Side,
        quantity: Decimal,
    ) -> Option<(Decimal, U256)> {
        let row = self.0.get(&(input, output))?;
        let mut left = quantity;
        let mut sum = Decimal::ZERO;
        for &(volume, price) in &row.levels {
            let buy = side == Side::Buy;
            let cap = if buy {
                volume.checked_mul(price)?
            } else {
                volume
            };
            let part = left.min(cap);
            sum = sum.checked_add(if buy {
                part.checked_div(price)?
            } else {
                part.checked_mul(price)?
            })?;
            left = left.checked_sub(part)?;
            if left.is_zero() {
                return Some((sum, row.minimum));
            }
        }
        None
    }
    fn markets(&self, quote: Address, p: &Parts) -> BTreeMap<Address, (bool, bool)> {
        let known:std::collections::HashSet<_>=p.tokens.all().into_iter().map(|t|t.address).collect();
        let mut result = BTreeMap::new();
        for (i, o) in self.0.keys().copied() {
            let coin = if i == quote {
                o
            } else if o == quote {
                i
            } else {
                continue;
            };
            if !known.contains(&coin) {
                continue;
            }
            let entry = result.entry(coin).or_insert((false, false));
            if i == quote {
                entry.0 = true
            } else {
                entry.1 = true
            }
        }
        result
    }
    pub(super) fn breaks(&self, i: Address, o: Address, side: Side, max: Decimal) -> Vec<Decimal> {
        let mut sum = Decimal::ZERO;
        self.0
            .get(&(i, o))
            .into_iter()
            .flat_map(|r| &r.levels)
            .map_while(|&(q, v)| {
                sum = sum.checked_add(if side == Side::Buy {
                    q.checked_mul(v)?
                } else {
                    q
                })?;
                (sum <= max).then_some(sum)
            })
            .collect()
    }
}
#[derive(Default)]
struct Inner {
    depth: Option<(u64, Instant, Arc<Depth>)>,
    tickets: HashMap<(Address, B256), Arc<super::routing::Ticket>>,
}
#[derive(Clone, Default)]
pub(in crate::venues::bsc_dex) struct Cache {
    inner: Arc<Mutex<Inner>>,
    pub(super) funds: availability::Cache,
}
impl Cache {
    pub(super) fn depth(&self, p: &Parts) -> Option<Arc<Depth>> {
        let age = p.ctx.int("native_rfq.max_age_ms").ok()?;
        let g = self.inner.lock().ok()?;
        let (rev, when, book) = g.depth.as_ref()?;
        (*rev == p.ctx.settings.generation() && when.elapsed() < Duration::from_millis(age))
            .then(|| book.clone())
    }
    pub(super) fn save(&self, t: super::routing::Ticket) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .tickets
            .retain(|_, q| q.expires() > Utc::now().timestamp().max(0) as u64);
        guard.tickets.insert(t.id(), Arc::new(t));
    }
    pub(in crate::venues::bsc_dex) fn take(
        &self,
        w: Address,
        id: B256,
    ) -> Option<Arc<super::routing::Ticket>> {
        self.inner.lock().ok()?.tickets.remove(&(w, id))
    }
}

pub(in crate::venues::bsc_dex) async fn clear(p: &Parts, out: &mpsc::Sender<MarketEvent>) {
    let had = p
        .native_quotes
        .inner
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .depth
        .take()
        .is_some();
    if had {
        let _ = out
            .send(MarketEvent::RfqMarkets {
                source: SourceId::new("Native RFQ"),
                venue: p.ctx.venue(),
                quote: Address::ZERO,
                markets: Default::default(),
                expires_ms: 0,
            })
            .await;
    }
}
pub(in crate::venues::bsc_dex) async fn run(p: Parts, out: mpsc::Sender<MarketEvent>) {
    let mut settings = p.ctx.settings.subscribe();
    loop {
        let initial = p.ctx.settings.generation();
        let cfg = || {
            let profile=Profile::read(&p.ctx).ok()?;
            profile.module?;
            Some((
                profile,
                p.ctx.int("native_rfq.max_age_ms").ok()?.max(1),
                p.tokens.usdt(&p.ctx).ok()?,
            ))
        };
        let Some((profile, age, quote)) = super::enabled(&p.ctx).then(cfg).flatten() else {
            clear(&p, &out).await;
            if matches!(
                settings.recv().await,
                Err(tokio::sync::broadcast::error::RecvError::Closed)
            ) {
                return;
            }
            continue;
        };
        let start = Instant::now();
        let stamp = Utc::now().timestamp_millis();
        let get = async {
            let mut url = super::client::url(&p.ctx, "orderbook").ok()?;
            url.query_pairs_mut().append_pair("chain", &profile.slug);
            let r = p.ctx.http.get(url);
            let (status, data) = http::exchange(r, p.ctx.limits().ok()?).await.ok()?;
            if !status.is_success() {
                return None;
            }
            Some(Arc::new(Depth::parse(
                &serde_json::from_str::<Value>(&data).ok()?,
            )?))
        };
        let book = tokio::time::timeout(Duration::from_millis(age), get)
            .await
            .ok()
            .flatten();
        if initial != p.ctx.settings.generation() {
            continue;
        }
        if let Some(b) = book {
            let markets = b.markets(quote, &p);
            p.native_quotes
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .depth = Some((initial, start, b));
            if out
                .send(MarketEvent::RfqMarkets {
                    source: SourceId::new("Native RFQ"),
                    venue: p.ctx.venue(),
                    quote,
                    markets,
                    expires_ms: stamp.saturating_add(i64::try_from(age).unwrap_or(i64::MAX)),
                })
                .await
                .is_err()
            {
                return;
            }
        } else {
            clear(&p, &out).await
        }
        tokio::select! {_ =tokio::time::sleep(Duration::from_millis((age/2).max(1)))=>{},c=settings.recv()=>if matches!(c,Err(tokio::sync::broadcast::error::RecvError::Closed)){return}}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_orderbook_has_two_independent_cash_directions_for_each_pair() {
        let packed = include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/native_rfq_ethereum_book.json.gz"
        );
        let fixture: Value =
            serde_json::from_reader(flate2::read::GzDecoder::new(packed.as_slice())).unwrap();
        let book = Depth::parse(&fixture["reply"]).unwrap();
        assert_eq!(book.0.len(), 6);
        for (&(i, o), row) in &book.0 {
            assert!(book.0.contains_key(&(o, i)));
            let (q, price) = row.levels[0];
            let (out, min) = book.value(i, o, Side::Sell, q).unwrap();
            assert_eq!(out, q.checked_mul(price).unwrap());
            assert_eq!(min, row.minimum);
            let capacity = row.levels.iter().map(|(qty, _)| *qty).sum::<Decimal>();
            assert!(
                book.value(i, o, Side::Sell, capacity + Decimal::ONE)
                    .is_none()
            );
        }
    }
    #[test]
    fn incremental_volume_and_raw_minimum_are_independent() {
        let mut book = Depth::default();
        let i = Address::repeat_byte(1);
        let o = Address::repeat_byte(2);
        book.0.insert(
            (i, o),
            Row {
                minimum: U256::from(91),
                levels: vec![
                    (Decimal::from(2), Decimal::from(3)),
                    (Decimal::from(2), Decimal::from(2)),
                ],
            },
        );
        assert_eq!(
            book.value(i, o, Side::Sell, Decimal::from(3)),
            Some((Decimal::from(8), U256::from(91)))
        );
        assert_eq!(
            book.value(i, o, Side::Buy, Decimal::from(8)),
            Some((Decimal::from(3), U256::from(91)))
        );
        assert!(book.value(i, o, Side::Buy, Decimal::from(11)).is_none());
    }
}
