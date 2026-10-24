//! Файл: venues/bsc_dex/directory/mod.rs
//! Что делает: справочник пулов от индексатора (решение 58) — общее состояние: очередь запросов к индексатору,
//! список DEX индексатора, пулы по каждой монете (как отдал индексатор) и итог адаптера по каждому пулу.
//! Индексатор — только справочник адресов: цены и состояние пулов адаптер берёт со своего узла.
//! Кто вызывает: ctx.rs (поле index), directory/* (заполняют), mod.rs (pool_directory для интерфейса), pools/*.
//! От чего зависит: core/types (directory.rs), node/pace.rs (очередь), gt_parse.rs.
//! Настройки: index.* [bsc_dex] — docs/SETTINGS.md, раздел «Справочник пулов».

pub mod check;
pub mod fetch;
pub mod gt_http;
pub mod gt_parse;
pub mod limit;
pub mod refresh;
pub mod store;
pub mod view;

pub use view::snapshot;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, RwLock};

use alloy_primitives::Address;
use chrono::{DateTime, Utc};

use crate::core::types::{DirectoryStatus, IndexerDex};

use super::node::pace::Pace;
use super::pools::discover::Found;
use crate::core::types::PoolRef;
use gt_parse::IndexedPool;

/// Монета справочника.
#[derive(Debug, Clone, Default)]
pub struct TokenEntry {
    pub ticker: String,
    pub fetched_at: Option<DateTime<Utc>>,
    /// Когда спрашивали последний раз (и при ошибке): срок следующего запроса считается от него.
    pub tried_at: Option<DateTime<Utc>>,
    pub pages: u32,
    pub pools: Vec<IndexedPool>,
    pub error: Option<String>,
}

/// Состояние справочника (под замком).
#[derive(Debug, Default)]
pub struct Snap {
    pub dexes: Vec<IndexerDex>,
    pub dexes_at: Option<DateTime<Utc>>,
    /// Когда список DEX спрашивали последний раз (и при ошибке).
    pub dexes_tried: Option<DateTime<Utc>>,
    pub tokens: HashMap<Address, TokenEntry>,
    /// Встречные токены подписки: USDT и промежуточные монеты — показываются только пулы с ними.
    pub quotes: HashSet<Address>,
    /// Итог адаптера по пулу (ключ — адрес пула или PoolId как у индексатора). Нет записи — «не проверен».
    pub status: HashMap<String, DirectoryStatus>,
    /// Пулы, прошедшие проверку на своём узле (ключ — как в status): их слушает подписка (pools/index.rs).
    pub taken: HashMap<String, Found>,
    /// Отказы, уже записанные в журнал (пул и причина) — каждый один раз.
    pub logged: HashSet<String>,
    /// Пулы, исключённые проверкой «кто меняет» (Universal Router меняет в другом пуле, check/emitter.rs): причина.
    pub banned: HashMap<PoolRef, String>,
    /// Пулы из событий создания (не от индексатора) по монете — тоже в справочнике и в проверке (pools/index.rs).
    pub events: HashMap<Address, Vec<IndexedPool>>,
    /// DEX индексатора с пулами, но не подключённые — для одной записи журнала на смену списка.
    pub unconnected: BTreeSet<String>,
    pub error: Option<String>,
}

/// Справочник пулов. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Directory {
    /// Очередь запросов к индексатору (не чаще index.requests_per_min) — отдельная от очереди узла.
    pub pace: Pace,
    pub snap: Arc<RwLock<Snap>>,
}

impl Directory {
    /// Изменение состояния под замком. Принимает: функцию. Возвращает: её результат. Ошибок нет.
    pub fn with<T>(&self, f: impl FnOnce(&mut Snap) -> T) -> T {
        f(&mut self.snap.write().unwrap_or_else(|p| p.into_inner()))
    }

    /// Причина исключения пула проверкой «кто меняет». None — не исключён. Ошибок нет.
    pub fn banned(&self, r: &PoolRef) -> Option<String> {
        self.snap
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .banned
            .get(r)
            .cloned()
    }

    /// Исключить пул: Universal Router его DEX меняет в другом пуле (check/emitter.rs). Пул снимается со «взятых»,
    /// в справочнике — «отклонён» с причиной. Возвращает: true — исключён впервые (для одной записи журнала).
    pub fn ban(&self, r: &PoolRef, why: &str) -> bool {
        self.with(|s| {
            let key = pool_key(r);
            s.taken.remove(&key);
            s.status
                .insert(key, DirectoryStatus::Rejected(why.to_string()));
            s.banned.insert(r.clone(), why.to_string()).is_none()
        })
    }

    /// Пулы, которые нужны подписке: пара «монета — USDT / промежуточная» по данным индексатора.
    /// Возвращает: (монета, по которой спрашивали, пул). Ошибок нет.
    pub fn relevant(&self) -> Vec<(Address, IndexedPool)> {
        let s = self.snap.read().unwrap_or_else(|p| p.into_inner());
        let mut out = Vec::new();
        let events = s.events.iter().map(|(a, l)| (a, l.as_slice()));
        let lists = s.tokens.iter().map(|(a, t)| (a, t.pools.as_slice()));
        for (token, list) in lists.chain(events) {
            for p in list {
                if is_relevant(*token, p, &s.quotes) {
                    out.push((*token, p.clone()));
                }
            }
        }
        out
    }
}

/// Ключ пула в справочнике — как его пишет индексатор: адрес V2/V3 или PoolId V4, 0x и строчные. Ошибок нет.
pub fn pool_key(r: &PoolRef) -> String {
    match r {
        PoolRef::Contract(a) => format!("{a:#x}"),
        PoolRef::Singleton { id, .. } => format!("{id}"),
    }
}

/// Причина для пулов с нативным BNB.
pub const NATIVE: &str = "валюта пула — нативный BNB (адрес 0x0): такие пулы пока не поддержаны";

/// Пул монеты с нативным BNB (валюта 0x0 — так её отдаёт индексатор у пулов V4/Infinity): показывается со статусом
/// «не поддержан», не прячется. Ошибок нет.
pub fn is_native(token: Address, p: &IndexedPool) -> bool {
    (p.base == token && p.quote == Address::ZERO) || (p.quote == token && p.base == Address::ZERO)
}

/// Нужен ли пул подписке: один токен пула — монета, по которой спрашивали, другой — USDT или промежуточная
/// (и это не та же монета). Ошибок нет.
pub fn is_relevant(token: Address, p: &IndexedPool, quotes: &HashSet<Address>) -> bool {
    let other = if p.base == token {
        p.quote
    } else if p.quote == token {
        p.base
    } else {
        return false;
    };
    other != token && quotes.contains(&other)
}
