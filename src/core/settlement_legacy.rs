//! Восстановление единиц старого withdraw_fact по тому же выводу.
use super::{Basis, Modes, Observation, ReceiptKey};
use crate::core::{types::VenueId, withdraw_seen::Seen};
use rust_decimal::Decimal;
use std::collections::{HashMap, hash_map::Entry};

// В старом withdraw_fact отсутствуют хеш и получатель. Совпадение чисел у двух
// исторических выводов делает его непригодным для раздельного учёта.
type Key = (VenueId, Decimal, Decimal, Decimal);
#[derive(Default, PartialEq)]
pub(in crate::core) struct Legacy(HashMap<Key, Option<(VenueId, Decimal, Decimal)>>);

pub(in crate::core) enum Interpretation {
    Unknown,
    ThisRoute(Seen),
    OtherRoute,
}

impl Legacy {
    pub(super) fn from_receipts(
        rows: &[Observation],
        receipts: &HashMap<ReceiptKey, (Decimal, Decimal)>,
        modes: &Modes,
        scale: Decimal,
    ) -> Self {
        let mut out = Self::default();
        for row in rows {
            let Some(d) = &row.debit else { continue };
            let key = (
                row.dex.clone(),
                row.wallet.clone(),
                true,
                row.hash.clone(),
                row.raw,
            );
            let candidate = receipts.get(&key).and_then(|&(ui, historical)| {
                (modes.get(&(row.cex.clone(), true)) == Some(&Basis::Display)
                    && historical == scale
                    && ui != row.raw
                    && ui > Decimal::ZERO
                    && ui <= row.exchange)
                    .then(|| (row.dex.clone(), d.reported, ui))
            });
            for gross in [Some(d.reported), (d.held != d.reported).then_some(d.held)]
                .into_iter()
                .flatten()
            {
                match out.0.entry((row.cex.clone(), gross, d.fee, row.raw)) {
                    Entry::Vacant(v) => {
                        v.insert(candidate.clone());
                    }
                    Entry::Occupied(mut o) => {
                        o.insert(None);
                    }
                }
            }
        }
        out
    }

    pub(in crate::core) fn interpreted(&self, seen: &Seen, to: Option<&VenueId>) -> Interpretation {
        let Some(fee) = seen.fee else {
            return Interpretation::Unknown;
        };
        let key = (seen.from.clone(), seen.sent, fee, seen.credited);
        match self.0.get(&key).and_then(Option::as_ref) {
            Some((dex, sent, credited)) if Some(dex) == to => Interpretation::ThisRoute(Seen {
                sent: *sent,
                credited: *credited,
                ..seen.clone()
            }),
            Some(_) => Interpretation::OtherRoute,
            None => Interpretation::Unknown,
        }
    }
}
