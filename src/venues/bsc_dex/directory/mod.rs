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

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, RwLock};

use alloy_primitives::Address;
use chrono::{DateTime, Utc};

use crate::core::types::{
    DirectoryPool, DirectoryStatus, DirectoryToken, IndexerDex, PoolDirectory,
};

use super::node::pace::Pace;
use super::pools::discover::Found;
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

    /// Пулы, которые нужны подписке: пара «монета — USDT / промежуточная» по данным индексатора.
    /// Возвращает: (монета, по которой спрашивали, пул). Ошибок нет.
    pub fn relevant(&self) -> Vec<(Address, IndexedPool)> {
        let s = self.snap.read().unwrap_or_else(|p| p.into_inner());
        let mut out = Vec::new();
        for (token, t) in &s.tokens {
            for p in &t.pools {
                if is_relevant(*token, p, &s.quotes) {
                    out.push((*token, p.clone()));
                }
            }
        }
        out
    }

    /// Справочник для интерфейса: источник, DEX, монеты, пулы с итогом. Принимает: источник, сеть, подпись
    /// (из настроек, может не быть). Ошибок нет.
    pub fn view(
        &self,
        source: Option<String>,
        network: Option<String>,
        attr: (Option<String>, Option<String>),
    ) -> PoolDirectory {
        let s = self.snap.read().unwrap_or_else(|p| p.into_inner());
        let mut tokens = Vec::new();
        let mut pools = Vec::new();
        for (addr, t) in &s.tokens {
            tokens.push(DirectoryToken {
                token: *addr,
                ticker: t.ticker.clone(),
                fetched_at: t.fetched_at,
                pages: t.pages,
                total: t.pools.len(),
                error: t.error.clone(),
            });
            let shown = |p: &&IndexedPool| is_relevant(*addr, p, &s.quotes) || is_native(*addr, p);
            for p in t.pools.iter().filter(shown) {
                let status = if is_native(*addr, p) {
                    DirectoryStatus::Unsupported(NATIVE.into())
                } else {
                    s.status
                        .get(&p.pool)
                        .cloned()
                        .unwrap_or(DirectoryStatus::Unchecked)
                };
                pools.push(DirectoryPool {
                    token: *addr,
                    dex_id: p.dex_id.clone(),
                    pool: p.pool.clone(),
                    base: p.base,
                    quote: p.quote,
                    base_symbol: p.base_symbol.clone(),
                    quote_symbol: p.quote_symbol.clone(),
                    reserve_usd: p.reserve_usd,
                    status,
                });
            }
        }
        tokens.sort_by(|a, b| a.ticker.cmp(&b.ticker));
        PoolDirectory {
            source,
            network,
            attribution_text: attr.0,
            attribution_url: attr.1,
            dexes: s.dexes.clone(),
            dexes_at: s.dexes_at,
            tokens,
            pools,
            error: s.error.clone(),
        }
    }
}

/// Справочник для интерфейса (контракт pool_directory) с источником и подписью из настроек. Ошибок нет.
pub fn snapshot(ctx: &super::ctx::Ctx) -> PoolDirectory {
    let attr = (
        ctx.text("index.attribution_text").ok(),
        ctx.text("index.attribution_url").ok(),
    );
    let source = ctx.text("index.api_url").ok();
    ctx.index.view(source, ctx.text("index.network").ok(), attr)
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
