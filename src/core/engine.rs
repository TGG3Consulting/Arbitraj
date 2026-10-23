//! Файл: core/engine.rs
//! Что делает: ядро целиком — создание общего состояния (inner.rs), загрузка учёта и запуск фоновых
//! задач: приём событий адаптеров, пересборка справочника, обновление сетей бирж, очередь расчёта, сверка.
//! Кто вызывает: main.rs (Core::start), api и notify (чтение состояния, живой поток, команды — методы Core).
//! От чего зависит: deps.rs, inner.rs, tick.rs, rebuild.rs, registry_sync.rs, candidates.rs, reconcile.rs,
//! ledger.rs, gas_stats.rs, manual.rs.
//! Настройки: у модулей свои (docs/SETTINGS.md).

use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

use tokio::sync::mpsc;

use crate::gateway::contract::VenueKind;

use super::deps::Deps;
pub use super::inner::{Inner, TransitKey};
use super::registry::CoinEntry;
use super::signal::SpreadView;
use super::view::CoreEvent;

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
            inner: Arc::new(Inner::new(deps, kinds.clone(), market_tx, cand_tx)),
        };
        // Учёт ядра: сделки, статистика газа, ручное управление — до запуска задач.
        super::ledger::load(&core).await;
        super::gas_stats::load(&core).await;
        super::manual::load(&core).await;
        super::transit::load(&core).await;
        tokio::spawn(super::tick::run(core.clone(), market_rx));
        tokio::spawn(super::rebuild::run(core.clone()));
        tokio::spawn(super::candidates::run(core.clone(), cand_rx));
        tokio::spawn(super::reconcile::run(core.clone()));
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
