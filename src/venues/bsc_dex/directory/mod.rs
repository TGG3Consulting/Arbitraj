//! Файл: venues/bsc_dex/directory/mod.rs
//! Что делает: справочник пулов от индексатора (решение 58) — общее состояние: очередь запросов к индексатору,
//! список DEX индексатора, пулы по каждой монете (как отдал индексатор; из событий создания; найденные фабриками на
//! своём узле — решение 121, C3) и итог адаптера по каждому пулу.
//! Индексатор — только справочник адресов: цены и состояние пулов адаптер берёт со своего узла.
//! Кто вызывает: ctx.rs (поле index), directory/* (заполняют), mod.rs (pool_directory для интерфейса), pools/*.
//! От чего зависит: core/types (directory.rs), node/pace.rs (очередь), gt_parse.rs.
//! Настройки: index.* [bsc_dex] — docs/SETTINGS.md, раздел «Справочник пулов».

pub mod banned_now;
pub mod check;
pub mod due;
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
    /// Сведение «индексатор не знает пулов монеты» (404, решение 121, C2) — не ошибка.
    pub note: Option<String>,
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
    /// Ключ — физический пул `PoolRef::site()` (решение 295; у парных пулов — сама ссылка).
    pub banned: HashMap<PoolRef, String>,
    /// Пулы из событий создания (не от индексатора) по монете — тоже в справочнике и в проверке (pools/index.rs).
    pub events: HashMap<Address, Vec<IndexedPool>>,
    /// Пулы, найденные фабриками на своём узле (решение 121, C3; pools/factory_scan.rs) по монете — второй источник
    /// справочника: та же проверка на узле и тот же реестр, что у пулов индексатора (pools/index.rs).
    pub factory: HashMap<Address, Vec<IndexedPool>>,
    /// Итог последнего прохода поиска по фабрикам — для интерфейса (taken считается при показе).
    pub factory_scan: crate::core::types::FactoryScan,
    /// DEX индексатора с пулами, но не подключённые — для одной записи журнала на смену списка.
    pub unconnected: BTreeSet<String>,
    pub error: Option<String>,
    /// Решение 295: DEX в режиме dex.multi_pair = да на последнем проходе take (ключи строк справочника — с парой,
    /// multi_pair::dir_key). Пусто — режим нигде не включён: ключи — строка пула, как до решения.
    pub multi_dex: HashSet<String>,
    /// Решение 295: монеты многомонетного пула (строка адреса пула → монеты), запомненные его проверкой в режиме.
    pub pool_tokens: HashMap<String, Vec<Address>>,
    /// Решение 474: пары «обёртка ERC4626 — её базовый токен» пула Balancer V3 (строка адреса пула → пары) — в
    /// развёртке пар не строятся (pools/verify_balancer_v3_buffer.rs twin).
    pub pool_twins: HashMap<String, Vec<(Address, Address)>>,
    /// Решение 295: развёрнутые пары многомонетных пулов по монете (multi_pair::expand) — источник справочника.
    pub multi: HashMap<Address, Vec<IndexedPool>>,
}

/// Справочник пулов. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Directory {
    /// Очередь запросов к индексатору (не чаще index.requests_per_min) — отдельная от очереди узла.
    pub pace: Pace,
    pub snap: Arc<RwLock<Snap>>,
    /// Решение 409 (t194, п. г): копия Snap.banned для горячего пути котировки (banned — на каждый пул) — снимок за
    /// ArcSwap без замка, причина Arc<str> (без копии строки). Пишется только в ban, под замком snap, вслед за banned.
    banned_now: Arc<banned_now::BannedNow>,
}

impl Directory {
    /// Изменение состояния под замком. Принимает: функцию. Возвращает: её результат. Ошибок нет.
    pub fn with<T>(&self, f: impl FnOnce(&mut Snap) -> T) -> T {
        f(&mut self.snap.write().unwrap_or_else(|p| p.into_inner()))
    }

    /// Причина исключения пула проверкой «кто меняет». None — не исключён. Решение 409: из снимка banned_now — без
    /// замка справочника и без копии строки. Ошибок нет.
    pub fn banned(&self, r: &PoolRef) -> Option<Arc<str>> {
        self.banned_now.get(&r.site())
    }

    /// Исключить пул: Universal Router его DEX меняет в другом пуле (check/emitter.rs). Пул снимается со «взятых»,
    /// в справочнике — «отклонён» с причиной. Возвращает: true — исключён впервые (для одной записи журнала).
    /// Решение 295 (ревью, 6): запрет — по физическому пулу (`PoolRef::site`): у пары многомонетного пула он действует
    /// на все пары контракта и на форму ссылки после смены dex.multi_pair; у прочих ссылок site() = ссылка — ключ прежний.
    pub fn ban(&self, r: &PoolRef, why: &str) -> bool {
        self.with(|s| {
            // Решение 305 (ревью): у пары WooFi (Member) ключ строки зависит от режима dex.multi_pair (с парой или строка
            // контракта) — статус пишется только под ключами её взятых записей (поиск по site ниже), не под pool_key:
            // в режиме «да» запись под строкой контракта была бы сиротой. Не взята — «отклонён» запишет record() по
            // banned при следующей проверке, под верным ключом.
            if !matches!(r, PoolRef::Member { .. }) {
                let key = pool_key(r);
                s.taken.remove(&key);
                s.status
                    .insert(key, DirectoryStatus::Rejected(why.to_string()));
            }
            // Остальные взятые пары того же контракта (только у Multi). Задумано: запрет — по контракту, поэтому снимается и
            // запись того же контракта под другим id DEX или с ссылкой Contract (site() тот же) — проверка «кто меняет»
            // осудила контракт, а не пару.
            // Решение 305: пара WooFi (Member) взята под ключом режима (multi_pair::dir_key) — снимается тем же поиском по
            // физическому пулу (у Member site() = сама пара), статус — под тем же ключом.
            if matches!(r, PoolRef::Multi { .. } | PoolRef::Member { .. }) {
                let site = r.site();
                let same: Vec<String> = s
                    .taken
                    .iter()
                    .filter(|(_, f)| f.reference.site() == site)
                    .map(|(k, _)| k.clone())
                    .collect();
                for k in same {
                    s.taken.remove(&k);
                    s.status
                        .insert(k, DirectoryStatus::Rejected(why.to_string()));
                }
            }
            let first = s.banned.insert(r.site(), why.to_string()).is_none();
            // Решение 409: снимок для banned — под тем же замком snap, вслед за Snap.banned (порядок записей тот же).
            self.banned_now.put(r.site(), why);
            first
        })
    }

    /// Пулы, которые нужны подписке: пара «монета — USDT / промежуточная» по данным индексатора, событий создания,
    /// поиска по фабрикам (решение 121, C3) и развёртки многомонетных пулов (решение 295).
    /// Возвращает: (монета, по которой спрашивали, пул). Ошибок нет.
    pub fn relevant(&self) -> Vec<(Address, IndexedPool)> {
        let s = self.snap.read().unwrap_or_else(|p| p.into_inner());
        let mut out = Vec::new();
        let events = s.events.iter().map(|(a, l)| (a, l.as_slice()));
        let factory = s.factory.iter().map(|(a, l)| (a, l.as_slice()));
        // Решение 295: развёрнутые пары многомонетных пулов (multi_pair::expand) — после прочих источников.
        let multi = s.multi.iter().map(|(a, l)| (a, l.as_slice()));
        let lists = s.tokens.iter().map(|(a, t)| (a, t.pools.as_slice()));
        for (token, list) in lists.chain(events).chain(factory).chain(multi) {
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
        // Пара общего контракта (решение 122 F, 3): индексатор и поиск по фабрике пишут адрес общего контракта.
        PoolRef::Member { contract, .. } => format!("{contract:#x}"),
        // Решение 295: пара многомонетного пула — ключ строки справочника с парой (тот же формат, что
        // multi_pair::dir_key у DEX в режиме dex.multi_pair).
        PoolRef::Multi {
            contract,
            token0,
            token1,
        } => format!("{contract:#x}|{token0:#x}|{token1:#x}"),
    }
}

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
