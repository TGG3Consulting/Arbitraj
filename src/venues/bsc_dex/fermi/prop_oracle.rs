//! Ценовые слоты конкретного контракта в публичном кадре Titan или BuilderNet.
use super::{Builder, Deployment, Frame};
use alloy_primitives::{Address, B256, U64, keccak256};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Oracle {
    pub(super) market: Address,
    pub(super) address: Address,
    pub(super) hash: B256,
    slots: BTreeSet<B256>,
}

impl Oracle {
    pub(super) fn parse(market: Address, rows: &[String]) -> Option<Self> {
        let [row] = rows else { return None };
        let mut fields = row.split_whitespace();
        let address: Address = fields.next()?.parse().ok()?;
        let hash: B256 = fields.next()?.parse().ok()?;
        let mut slots = BTreeSet::new();
        for s in fields {
            if !slots.insert(s.parse().ok()?) {
                return None;
            }
        }
        if market.is_zero()
            || address.is_zero()
            || hash.is_zero()
            || hash == keccak256([])
            || slots.is_empty()
            || slots.len() > 128
        {
            return None;
        }
        Some(Self {
            market,
            address,
            hash,
            slots,
        })
    }

    pub(super) fn frame(&self, builder: Builder, snapshot: &Value) -> Option<Frame> {
        let (row, number, slot) = match builder {
            Builder::Titan => {
                let mut rows = snapshot["venues"].as_array()?.iter().filter(|r| {
                    r["quoteAddress"]
                        .as_str()
                        .and_then(|s| s.parse::<Address>().ok())
                        == Some(self.market)
                });
                let r = rows.next()?;
                if rows.next().is_some() {
                    return None;
                }
                (r, r["blockNumber"].as_u64()?, r["slot"].as_u64()?)
            }
            Builder::BuilderNet => (
                snapshot.get(format!("{:#x}", self.market))?,
                serde_json::from_value::<U64>(snapshot["blockNumber"].clone())
                    .ok()?
                    .to(),
                snapshot["slot"].as_u64()?,
            ),
        };
        for (account, state) in row["stateOverride"].as_object()? {
            if state.get("code").is_some() || state.get("state").is_some() {
                return None;
            }
            if let Some(d) = state.get("stateDiff") {
                let updates = d.as_object()?;
                if !updates.is_empty() && account.parse::<Address>().ok() != Some(self.address) {
                    return None;
                }
                if updates.keys().any(|s| {
                    s.parse::<B256>()
                        .map_or(true, |key| !self.slots.contains(&key))
                }) {
                    return None;
                }
            }
        }
        // Frame copies only stateDiff at the accepted oracle. Account nonces and balances remain canonical.
        Frame::from_row(
            row,
            number,
            slot,
            Deployment {
                router: self.market,
                engine: self.address,
                runtime: self.hash,
            },
        )
    }
}

#[cfg(test)]
#[path = "prop_oracle_tests.rs"]
mod tests;
