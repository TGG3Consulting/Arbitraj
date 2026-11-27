//! Ключи денежных шагов PancakeSwap X, для которых мог быть опубликован ордер.
//! При открытии учёта читаются только брони nonce Permit2 этого канала по первичному ключу core_state.
//! В обычной сети без таких броней проверка prior заканчивается до построения ключа сделки.
use arc_swap::ArcSwap;
use rusqlite::Connection;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

#[derive(Default)]
pub(crate) struct Index {
    chains: ArcSwap<HashMap<u64, Arc<Orders>>>,
    damaged: HashSet<u64>,
    unknown_chain: bool,
}

#[derive(Default)]
pub(crate) struct Orders(RwLock<HashSet<String>>);

/// Не даёт конкурентному повтору выбрать другую площадку между записью намерения и отправкой.
pub(crate) struct Writing {
    orders: Arc<Orders>,
    key: Option<String>,
}

impl Index {
    /// У каждой подписи, переданной серверу, уже есть постоянная бронь [ключ поручения, digest].
    pub(crate) fn restore(c: &Connection) -> rusqlite::Result<Self> {
        let mut chains = HashMap::<u64, Arc<Orders>>::new();
        let mut damaged = HashSet::new();
        let mut unknown_chain = false;
        let mut q = c.prepare("SELECT id,body FROM core_state WHERE kind='pancakex_permit'")?;
        let mut rows = q.query([])?;
        while let Some(row) = rows.next()? {
            let json = |i| {
                row.get_ref(i)
                    .ok()?
                    .as_str()
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            };
            let slot = json(0);
            let chain = slot.as_ref().and_then(|v| v[0].as_u64());
            let valid_slot = slot.is_some_and(|v| {
                serde_json::from_value::<(
                    u64,
                    alloy_primitives::Address,
                    alloy_primitives::Address,
                    alloy_primitives::U256,
                )>(v)
                .is_ok()
            });
            let claim = json(1);
            let key = claim
                .as_ref()
                .and_then(|v| v[0].as_str())
                .filter(|s| !s.is_empty());
            let digest = claim
                .as_ref()
                .and_then(|v| v[1].as_str())
                .and_then(|s| s.parse::<alloy_primitives::B256>().ok());
            if let Some(chain) = chain
                && (!valid_slot || digest.is_none())
            {
                damaged.insert(chain);
            }
            match (chain, key) {
                (Some(chain), Some(key)) => Self::remember(&mut chains, chain, key.to_owned()),
                (Some(chain), None) => {
                    damaged.insert(chain);
                }
                (None, _) => unknown_chain = true,
            }
        }
        if unknown_chain || !damaged.is_empty() {
            // Повреждена бронь. Исходные ключи ищутся один раз в сохранённых поручениях.
            // Свежие подписи спорного домена запрещены; прочие DEX продолжают обычную работу.
            let mut q = c.prepare("SELECT id, json_extract(body,'$.lookup.parse.pancakex.ticket.cfg.p.chain')
                FROM core_state WHERE kind IN ('settlement_action','base_settlement_action')
                AND CASE WHEN json_valid(body) THEN json_type(body,'$.lookup.parse.pancakex')='object' ELSE 0 END")?;
            let mut rows = q.query([])?;
            while let Some(row) = rows.next()? {
                if let (Ok(chain), Ok(key)) = (row.get::<_, u64>(1), row.get::<_, String>(0)) {
                    Self::remember(&mut chains, chain, key);
                }
            }
        }
        Ok(Self {
            chains: ArcSwap::from_pointee(chains),
            damaged,
            unknown_chain,
        })
    }

    fn remember(chains: &mut HashMap<u64, Arc<Orders>>, chain: u64, key: String) {
        chains
            .entry(chain)
            .or_default()
            .0
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key);
    }

    pub(crate) fn unavailable() -> Self {
        Self {
            unknown_chain: true,
            ..Self::default()
        }
    }

    pub(crate) fn orders(&self, chain: impl FnOnce() -> Option<u64>) -> Option<Arc<Orders>> {
        let chains = self.chains.load();
        if chains.is_empty() {
            return None;
        }
        chains.get(&chain()?).cloned()
    }

    pub(crate) fn begin(&self, chain: u64, key: &str) -> Option<Writing> {
        if self.unknown_chain || self.damaged.contains(&chain) {
            return None;
        }
        let orders = self.orders(|| Some(chain)).unwrap_or_else(|| {
            let added = Arc::<Orders>::default();
            let previous = self.chains.rcu(|old| {
                let mut next = (**old).clone();
                next.entry(chain).or_insert_with(|| added.clone());
                Arc::new(next)
            });
            previous.get(&chain).cloned().unwrap_or(added)
        });
        if !orders
            .0
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_owned())
        {
            return None;
        }
        Some(Writing {
            orders,
            key: Some(key.to_owned()),
        })
    }
}

impl Orders {
    pub(crate) fn contains(&self, key: &str) -> bool {
        self.0
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(key)
    }
}

impl Writing {
    pub(crate) fn recorded(mut self) {
        self.key = None;
    }
}

impl Drop for Writing {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.orders
                .0
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key);
        }
    }
}
