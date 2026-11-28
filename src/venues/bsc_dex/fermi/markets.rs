//! Книги котировок Fermi и PropAMM. Лента получает один публичный кадр на каждого
//! выбранного строителя. Котировщик и исполнитель читают книги с актуальным слотом и поколением настроек.
use super::super::{hashflow::availability, parts::Parts};
pub(super) use super::slot_config::Config;
use super::{Frame, Head};
use crate::gateway::events::{MarketEvent, SourceId};
use alloy::primitives::Address;
use chrono::Utc;
use rust_decimal::Decimal;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
#[derive(Default)]
pub(super) struct SideBook(pub(super) Vec<(Decimal, Decimal)>);
impl SideBook {
    pub(super) fn price(&self, qty: Decimal, buy: bool) -> Option<Decimal> {
        if qty <= Decimal::ZERO {
            return None;
        }
        let at = self.0.partition_point(|(q, _)| *q < qty);
        let (size, cash) = self.0.get(at)?;
        let marginal = cash.checked_div(*size)?;
        let conservative = if *size == qty || at == 0 {
            marginal
        } else {
            let (before, prev) = self.0[at - 1];
            let prev = prev.checked_div(before)?;
            if buy {
                prev.max(marginal)
            } else {
                prev.min(marginal)
            }
        };
        conservative.checked_mul(qty)
    }
    pub fn breaks(&self, max: Decimal) -> Vec<Decimal> {
        self.0
            .iter()
            .filter_map(|(q, _)| (*q <= max).then_some(*q))
            .collect()
    }
}
pub(super) struct Books {
    pub config: Config,
    pub head: Head,
    pub frame: Frame,
    pub revision: u64,
    pub prop: Option<super::super::exec::prop_wallet::Terms>,
    pub sides: BTreeMap<(Address, bool), SideBook>,
}
impl Books {
    pub fn current(&self) -> bool {
        self.frame.current(
            self.head.number.to(),
            self.head.timestamp.to(),
            Utc::now().timestamp().max(0) as u64,
        )
    }
}
#[derive(Default, Clone)]
pub(in crate::venues::bsc_dex) struct Cache {
    books: Arc<Mutex<Vec<Arc<Books>>>>,
    pub(super) funds: availability::Cache,
    pub(super) tickets: super::routing::Tickets,
}
impl Cache {
    pub(super) fn current(&self, p: &Parts) -> Vec<Arc<Books>> {
        let generation = p.ctx.settings.generation();
        self.books
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .iter()
            .filter(|b| b.revision == generation && b.current())
            .cloned()
            .collect()
    }
    pub(super) fn contains(&self, book: &Books) -> bool {
        self.books
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .iter()
            .any(|active| {
                active.revision == book.revision
                    && active.config.builder == book.config.builder
                    && active.config.executor == book.config.executor
                    && active.config.same_path(&book.config)
                    && active.head.hash == book.head.hash
                    && super::slot_config::same_frame(&active.frame, &book.frame)
            })
    }
    pub(super) fn replace(&self, new: Vec<Arc<Books>>) -> bool {
        let mut data = self.books.lock().unwrap_or_else(|x| x.into_inner());
        let had = !data.is_empty();
        *data = new;
        had
    }
}
pub(in crate::venues::bsc_dex) async fn clear(p: &Parts, out: &mpsc::Sender<MarketEvent>) {
    if p.slot_markets.replace(Vec::new()) {
        let _ = out
            .send(MarketEvent::RfqMarkets {
                source: SourceId::new("Fermi"),
                venue: p.ctx.venue(),
                quote: Address::ZERO,
                markets: Default::default(),
                expires_ms: 0,
            })
            .await;
    }
}
pub(in crate::venues::bsc_dex) use super::slot_feed::run;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interpolation_uses_worse_observed_unit_price_and_cannot_cross_depth() {
        let side = SideBook(vec![
            (Decimal::from(1), Decimal::from(4)),
            (Decimal::from(3), Decimal::from(18)),
        ]);
        assert_eq!(side.price(Decimal::from(2), true), Some(Decimal::from(12)));
        assert_eq!(side.price(Decimal::from(2), false), Some(Decimal::from(8)));
        assert_eq!(side.price(Decimal::from(4), true), None);
        assert_eq!(side.price(Decimal::ZERO, false), None);
        assert_eq!(side.breaks(Decimal::from(2)), vec![Decimal::ONE]);
    }
}
