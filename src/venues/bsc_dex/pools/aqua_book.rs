//! Прообразы стратегий и наблюдавшиеся валюты. Доступность проверяется rawBalances в момент чтения.
use super::aqua::{self, IAqua, Program, Roles};
use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, Bytes};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, RwLock};

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Book {
    rows: BTreeMap<B256, Record>,
    #[serde(skip)]
    by_token: BTreeMap<Address, BTreeSet<B256>>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    maker: Address,
    raw: Bytes,
    assets: BTreeSet<Address>,
}
#[derive(Clone)]
pub(in crate::venues::bsc_dex) struct Offer {
    pub program: Program,
    pub hash: B256,
    pub first: bool,
}
impl Book {
    pub(super) fn insert(&mut self, maker: Address, h: B256, raw: Bytes) -> bool {
        if self.rows.contains_key(&h) {
            return self.rows[&h].maker == maker;
        }
        if aqua::program(maker, h, &raw).is_none() {
            return false;
        }
        self.rows.insert(
            h,
            Record {
                maker,
                raw,
                assets: BTreeSet::new(),
            },
        );
        true
    }
    pub(super) fn tokens(&mut self, maker: Address, h: B256, tokens: &[Address]) -> bool {
        let Some(row) = self.rows.get_mut(&h).filter(|r| r.maker == maker) else {
            return false;
        };
        let mut changed = false;
        for token in tokens.iter().copied().filter(|t| !t.is_zero()) {
            if row.assets.insert(token) {
                self.by_token.entry(token).or_default().insert(h);
                changed = true;
            }
        }
        changed
    }
    pub(super) fn maker(&self, h: B256) -> Option<Address> {
        self.rows.get(&h).map(|r| r.maker)
    }
    pub(super) fn copy_assets(&self, h: B256, assets: &[Address], to: &mut Book) {
        if let Some(row) = self.rows.get(&h)
            && to.insert(row.maker, h, row.raw.clone())
        {
            to.tokens(row.maker, h, assets);
        }
    }
    pub(super) fn selection(&self, hashes: &BTreeSet<B256>) -> Book {
        Book {
            rows: hashes
                .iter()
                .filter_map(|h| self.rows.get(h).map(|r| (*h, r.clone())))
                .collect(),
            ..Book::default()
        }
    }
    pub(super) fn persist(&self) -> Result<Vec<(String, String)>, serde_json::Error> {
        self.rows
            .iter()
            .map(|(h, r)| Ok((format!("row:{h:#x}"), serde_json::to_string(r)?)))
            .collect()
    }
    pub(super) fn restore(&mut self, h: B256, body: &str) -> Option<()> {
        let record: Record = serde_json::from_str(body).ok()?;
        if !self.insert(record.maker, h, record.raw) {
            return None;
        }
        self.tokens(
            record.maker,
            h,
            &record.assets.into_iter().collect::<Vec<_>>(),
        );
        Some(())
    }
    pub(super) fn observe(&mut self, roles: Roles, log: &Log) -> Option<(B256, bool)> {
        let emitter = log.inner.address;
        if log.removed {
            return None;
        }
        let topic = log.topics().first().copied().unwrap_or_default();
        if emitter == roles.registry {
            if topic == IAqua::Shipped::SIGNATURE_HASH
                && let Ok(e) = IAqua::Shipped::decode_log_data(&log.inner.data)
                && e.app == roles.engine
            {
                let new = !self.rows.contains_key(&e.strategyHash);
                return self
                    .insert(e.maker, e.strategyHash, e.strategy)
                    .then_some((e.strategyHash, new));
            } else if topic == IAqua::Pushed::SIGNATURE_HASH
                && let Ok(e) = IAqua::Pushed::decode_log_data(&log.inner.data)
                && e.app == roles.engine
                && self.maker(e.strategyHash) == Some(e.maker)
            {
                return Some((
                    e.strategyHash,
                    self.tokens(e.maker, e.strategyHash, &[e.token]),
                ));
            }
            // Docked не перечисляет токены. Сохранённый прообраз по этому событию не уничтожается.
        } else if emitter == roles.engine
            && topic == IAqua::Swapped::SIGNATURE_HASH
            && let Ok(e) = IAqua::Swapped::decode_log_data(&log.inner.data)
            && self.maker(e.orderHash) == Some(e.maker)
        {
            return Some((
                e.orderHash,
                self.tokens(e.maker, e.orderHash, &[e.tokenIn, e.tokenOut]),
            ));
        }
        None
    }
    pub(super) fn hashes(&self) -> impl Iterator<Item = B256> + '_ {
        self.rows.keys().copied()
    }
    pub(super) fn assets(&self, h: B256) -> Option<&BTreeSet<Address>> {
        self.rows.get(&h).map(|r| &r.assets)
    }
    pub(super) fn pairs(
        &self,
        pair: (Address, Address),
    ) -> impl Iterator<Item = (B256, bool)> + '_ {
        let a = self.by_token.get(&pair.0);
        let b = self.by_token.get(&pair.1);
        let (short, long) = if a.map_or(0, BTreeSet::len) <= b.map_or(0, BTreeSet::len) {
            (a, b)
        } else {
            (b, a)
        };
        short
            .into_iter()
            .flatten()
            .filter(move |h| long.is_some_and(|l| l.contains(*h)))
            .flat_map(|h| [(*h, false), (*h, true)])
    }
    pub(super) fn offer(&self, id: B256, pair: (Address, Address)) -> Option<Offer> {
        let (h, first) = self
            .pairs(pair)
            .find(|(h, first)| aqua::edge(*h, pair, *first) == id)?;
        let r = self.rows.get(&h)?;
        Some(Offer {
            program: aqua::program(r.maker, h, &r.raw)?,
            hash: h,
            first,
        })
    }
}
type Namespace = (u64, Address, Address);
static BOOKS: LazyLock<RwLock<HashMap<Namespace, Arc<Book>>>> = LazyLock::new(Default::default);
pub(super) fn current(chain: u64, r: Roles) -> Arc<Book> {
    BOOKS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(chain, r.registry, r.engine))
        .cloned()
        .unwrap_or_default()
}
pub(super) fn publish(chain: u64, r: Roles, b: Book) {
    let mut books = BOOKS.write().unwrap_or_else(|e| e.into_inner());
    let current = books.entry((chain, r.registry, r.engine)).or_default();
    let book = Arc::make_mut(current);
    for (h, mut row) in b.rows {
        let assets = std::mem::take(&mut row.assets);
        let maker = row.maker;
        book.rows.entry(h).or_insert(row);
        book.tokens(maker, h, &assets.into_iter().collect::<Vec<_>>());
    }
}

pub(super) fn observed(chain: u64, roles: Roles, log: &Log) -> Option<B256> {
    let mut books = BOOKS.write().unwrap_or_else(|e| e.into_inner());
    let b = books
        .entry((chain, roles.registry, roles.engine))
        .or_default();
    Arc::make_mut(b).observe(roles, log).map(|(h, _)| h)
}

#[cfg(test)]
#[path = "aqua_book_tests.rs"]
mod tests;
