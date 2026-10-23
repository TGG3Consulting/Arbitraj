//! Файл: core/engine.rs
//! Что делает: ядро целиком — общее состояние (рынок, справочник, чёрный список, сигналы) и запуск
//! фоновых задач: приём событий адаптеров, пересборка справочника, обновление сетей бирж.
//! Кто вызывает: main.rs (Core::start), api (чтение состояния и живой поток — через методы Core).
//! От чего зависит: deps.rs, market_state.rs, registry.rs, registry_sync.rs, rebuild.rs, tick.rs, view.rs.
//! Настройки: у модулей свои (docs/SETTINGS.md).
//!
//! Блокировки — std::sync и никогда не держатся через await: тик не ждёт других задач.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

use alloy_primitives::Address;
use rust_decimal::Decimal;
use tokio::sync::{Notify, mpsc};

use crate::core::types::{Asset, CoinNetworkInfo, Direction, VenueId};
use crate::gateway::contract::VenueKind;
use crate::gateway::events::MarketEvent;

use super::blacklist::Blacklist;
use super::deps::Deps;
use super::limits::Exposure;
use super::market_state::MarketState;
use super::registry::CoinEntry;
use super::signal::{Signals, SpreadView};
use super::view::{CoreEvent, Live};
use super::vol::{BlockClock, Volatility};

/// Ключ истории времени в пути: монета, A, B.
pub type TransitKey = (Asset, VenueId, VenueId);

/// Общее состояние ядра.
pub struct Inner {
    pub deps: Deps,
    pub live: Live,
    pub market: RwLock<MarketState>,
    /// Справочник сетей каждой биржи (последний успешный coin_networks).
    pub networks: RwLock<HashMap<VenueId, Vec<CoinNetworkInfo>>>,
    /// Площадки реестра шлюза и их вид (заполняется при старте).
    pub kinds: RwLock<Vec<(VenueId, VenueKind)>>,
    /// Справочник монет и направлений.
    pub coins: RwLock<Vec<CoinEntry>>,
    /// Адрес USDT в BSC или причина, почему его нет.
    pub usdt: RwLock<Result<Address, String>>,
    pub blacklist: RwLock<Blacklist>,
    /// Пулы, исключённые по крючку (чтобы уведомлять только о новых).
    pub excluded_pools: Mutex<HashSet<(VenueId, String)>>,
    /// Сколько монет ждали проверки налогов при прошлой пересборке (для coin.check_done).
    pub not_checked: Mutex<usize>,
    pub signals: Mutex<Signals>,
    /// Волатильность цен по замерам сигнала (vol.rs).
    pub vol: Mutex<Volatility>,
    /// Среднее время блока BSC по событиям пулов (vol.rs).
    pub clock: Mutex<BlockClock>,
    pub spreads: RwLock<HashMap<Direction, SpreadView>>,
    /// На какие монеты подписана каждая площадка.
    pub subs: tokio::sync::Mutex<HashMap<VenueId, Vec<Asset>>>,
    /// Монеты, по которым идёт пересчёт тика: значение true — пришёл ещё тик, пересчитать снова.
    pub ticking: Mutex<HashMap<Asset, bool>>,
    /// Очередь тяжёлого расчёта (candidates.rs) и направления, уже стоящие в ней.
    pub cand_tx: mpsc::Sender<Direction>,
    pub queued: Mutex<HashSet<Direction>>,
    /// Последняя причина отказа по направлению (чтобы не повторять её в журнале).
    pub last_reject: Mutex<HashMap<Direction, String>>,
    /// История времени в пути: (монета, A, B) → (сумма секунд, число переводов). Ведёт transfers.rs.
    pub transits: Mutex<HashMap<TransitKey, (Decimal, u64)>>,
    /// Деньги в открытых сделках (limits.rs). Ведёт executor.rs.
    pub exposure: Mutex<Exposure>,
    /// Канал событий адаптеров (передаётся в subscribe_market).
    pub market_tx: mpsc::Sender<MarketEvent>,
    /// «Толчок» обновления сетей бирж (аномальный спред — перечитать статусы ввода и вывода).
    pub kick_networks: Notify,
    /// «Толчок» пересборки справочника (несколько толчков подряд сливаются в одну пересборку).
    pub kick_rebuild: Notify,
}

/// Ядро. Дешёво клонируется.
#[derive(Clone)]
pub struct Core {
    pub inner: Arc<Inner>,
}

/// Чтение под блокировкой (отравленная блокировка не роняет систему — данные берутся как есть).
pub fn rd<T>(l: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|p| p.into_inner())
}

/// Запись под блокировкой.
pub fn wr<T>(l: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(|p| p.into_inner())
}

/// Блокировка Mutex без паники на отравлении.
pub fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Core {
    /// Создаёт ядро и запускает фоновые задачи. Площадки уже зарегистрированы в шлюзе (main.rs).
    /// Принимает: зависимости. Возвращает: ядро. Ошибок нет — всё, что не так, пишется в журнал.
    pub async fn start(deps: Deps) -> Core {
        let (market_tx, market_rx) = mpsc::channel(deps.buffer);
        let (cand_tx, cand_rx) = mpsc::channel(deps.buffer);
        let mut kinds = vec![];
        for id in deps.gw.ids().await {
            if let Ok(v) = deps.gw.get(&id).await {
                kinds.push((id, v.kind()));
            }
        }
        kinds.sort_by(|a, b| a.0.0.cmp(&b.0.0));
        let core = Core {
            inner: Arc::new(Inner {
                live: Live::new(deps.buffer),
                deps,
                market: RwLock::default(),
                networks: RwLock::default(),
                kinds: RwLock::new(kinds.clone()),
                coins: RwLock::default(),
                usdt: RwLock::new(Err("справочник бирж ещё не загружен".into())),
                blacklist: RwLock::default(),
                excluded_pools: Mutex::default(),
                not_checked: Mutex::default(),
                signals: Mutex::default(),
                vol: Mutex::default(),
                clock: Mutex::default(),
                spreads: RwLock::default(),
                subs: tokio::sync::Mutex::default(),
                ticking: Mutex::default(),
                cand_tx,
                queued: Mutex::default(),
                last_reject: Mutex::default(),
                transits: Mutex::default(),
                exposure: Mutex::default(),
                market_tx,
                kick_networks: Notify::new(),
                kick_rebuild: Notify::new(),
            }),
        };
        tokio::spawn(super::tick::run(core.clone(), market_rx));
        tokio::spawn(super::rebuild::run(core.clone()));
        tokio::spawn(super::candidates::run(core.clone(), cand_rx));
        tokio::spawn(super::rebuild::watch_settings(core.clone()));
        for (id, kind) in kinds {
            if kind == VenueKind::Cex {
                tokio::spawn(super::registry_sync::run(core.clone(), id));
            }
        }
        core
    }

    /// Подписка интерфейса на живой поток ядра (api/ws.rs).
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<CoreEvent> {
        self.inner.live.subscribe()
    }

    /// Справочник монет (экран «Монеты и направления»).
    pub fn coins(&self) -> Vec<CoinEntry> {
        rd(&self.inner.coins).clone()
    }

    /// Последние строки «Живого экрана» по всем направлениям.
    pub fn spreads(&self) -> Vec<SpreadView> {
        rd(&self.inner.spreads).values().cloned().collect()
    }

    /// Просит пересобрать справочник (толчки подряд сливаются).
    pub fn request_rebuild(&self) {
        self.inner.kick_rebuild.notify_one();
    }
}
