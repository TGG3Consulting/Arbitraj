//! Файл: core/engine.rs
//! Что делает: ядро целиком — создание общего состояния (inner.rs), загрузка учёта и запуск фоновых
//! задач: приём событий адаптеров, пересборка справочника, обновление сетей бирж, делистинги бирж (решение 175),
//! очередь расчёта, сверка.
//! Кто вызывает: main.rs (Core::start), api и notify (чтение состояния, живой поток, команды — методы Core).
//! От чего зависит: deps.rs, inner.rs, tick.rs, rebuild.rs, registry_sync.rs, candidates.rs, reconcile.rs,
//! ledger.rs, gas_stats.rs, manual.rs.
//! Настройки: у модулей свои (docs/SETTINGS.md).

use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

use tokio::sync::mpsc;

use crate::gateway::contract::VenueKind;

use super::deps::Deps;
pub use super::inner::{Inner, TransitKey};
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
        // Итог дня — раньше сделок: прерванные перезапуском сделки сразу добавляют предварительный убыток.
        super::daily::load(&core).await;
        super::ledger::load(&core).await;
        // Решение 163: возвраты USDT «ждёт повтора» продолжают повторы после перезапуска.
        super::usdt_return_retry::resume(&core);
        super::gas_stats::load(&core).await;
        super::manual::load(&core).await;
        super::transit::load(&core).await;
        super::low_usdt::load(&core).await;
        // Задача эпизодов (решение 74): при запуске закрывает прерванные перезапуском, дальше — запись и уборка.
        if let Some(rx) = lk(&core.inner.episode_rx).take() {
            tokio::spawn(super::episode_worker::run(core.clone(), rx));
        }
        // Задача окон (решение 125): при запуске закрывает прерванные перезапуском, дальше — запись, закрытие, уборка.
        if let Some(rx) = lk(&core.inner.window_rx).take() {
            tokio::spawn(super::window_worker::run(core.clone(), rx));
        }
        tokio::spawn(super::tick::run(core.clone(), market_rx));
        tokio::spawn(super::rebuild::run(core.clone()));
        tokio::spawn(super::candidates::run(core.clone(), cand_rx));
        tokio::spawn(super::reconcile::run(core.clone()));
        tokio::spawn(super::rebuild::watch_settings(core.clone()));
        // Решение 124: исполнение циклов DEX ↔ DEX — одна задача, толчки событий пулов сливаются.
        tokio::spawn(super::cycle_watch::run(core.clone()));
        for (id, kind) in kinds {
            if kind == VenueKind::Cex {
                tokio::spawn(super::registry_sync::run(core.clone(), id.clone()));
                // Решение 202: сторож пар и соединений стаканов биржи.
                tokio::spawn(super::feed_watch::run(core.clone(), id.clone()));
                // Решение 175: делистинги биржи — фоном, delist.refresh_s.
                tokio::spawn(super::delist::run(core.clone(), id));
            }
        }
        core
    }

    /// Подписка интерфейса на живой поток ядра (api/ws.rs).
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<CoreEvent> {
        self.inner.live.subscribe()
    }

    /// Справочник монет (экран «Монеты и направления»).
    /// Решение 131: снимок за Arc — без копии записей; все монеты (рабочие и неподвижные, coin_book.rs).
    pub fn coins(&self) -> super::coin_book::CoinBook {
        rd(&self.inner.coins).clone()
    }

    /// Последние строки «Живого экрана» по всем направлениям.
    pub fn spreads(&self) -> Vec<SpreadView> {
        rd(&self.inner.spreads).values().cloned().collect()
    }

    /// Делистинги бирж (решение 175): по площадке — последний ответ, его время и последняя ошибка (GET /api/delistings).
    pub fn delistings(&self) -> Vec<(crate::core::types::VenueId, super::delist::VenueDelist)> {
        self.inner.delist.all()
    }

    /// Просит пересобрать справочник (толчки подряд сливаются).
    pub fn request_rebuild(&self) {
        self.inner.kick_rebuild.notify_one();
    }

    /// Решение 183 (Тигран, 05.11.2026): секреты разблокированы или изменены. Пока они закрыты, подписанные запросы
    /// бирж (справочник сетей, подписка площадки) не проходят, сбой запоминается, а повтор шёл только по следующему
    /// толчку — изменению любой настройки или своему интервалу опроса; биржа, не дождавшаяся разблокировки при
    /// запуске, так и оставалась без подписки. Теперь: справочники сетей всех бирж перечитываются сразу
    /// (registry_sync.rs), пересборка повторяет каждую неудавшуюся подписку (sub_state.rs, subscribe.rs). Удавшиеся
    /// подписки не трогаются. Ошибок нет.
    pub fn secrets_changed(&self) {
        self.inner.kick_networks.notify_waiters();
        self.request_rebuild();
    }
}
