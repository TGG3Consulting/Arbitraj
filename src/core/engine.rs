//! Файл: core/engine.rs
//! Что делает: ядро целиком — создание общего состояния (inner.rs), загрузка учёта и запуск фоновых
//! задач: приём событий адаптеров, пересборка справочника, обновление сетей бирж, делистинги бирж (решение 175),
//! очередь расчёта, сверка.
//! Кто вызывает: main.rs (Core::start), api и notify (чтение состояния, живой поток, команды — методы Core).
//! От чего зависит: deposit_min_ask.rs (решение 387, forget), deps.rs, inner.rs, tick.rs, rebuild.rs, registry_sync.rs, candidates.rs, reconcile.rs,
//! ledger.rs, gas_stats.rs, manual.rs, deposit_warm.rs (решение 316), book_marks_on.rs.
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
        // Решение 242, дополнение: виды площадок — хранилищу настроек для проверки пары «площадка · тип направления»
        // при записи (settings/venue_kinds.rs; те же виды, по которым правило стороны core/venue_dir.rs выбирает пару).
        deps.settings
            .venue_kinds()
            .set(kinds.iter().map(|(id, k)| (id.0.clone(), *k)));
        let core = Core {
            inner: Arc::new(Inner::new(deps, kinds.clone(), market_tx, cand_tx)),
        };
        // Решение 432: имена площадок DEX для человека («DEX» при одной сети, «DEX <сеть>» при нескольких).
        super::chain_run::publish_names(&core);
        // Решение 251: шаблон алерта Telegram берёт карточку сделки (путь, где сейчас) по id из учёта — свежую на
        // момент отправки (notify/alert_card.rs). Слабая ссылка: ядро не держится уведомлениями.
        let weak = Arc::downgrade(&core.inner);
        crate::notify::alert_card::install(move |id| {
            let inner = weak.upgrade()?;
            lk(&inner.deals).get(id).map(super::deal_card::card)
        });
        // Учёт ядра: сделки, статистика газа, ручное управление — до запуска задач.
        // Итог дня — раньше сделок: прерванные перезапуском сделки сразу добавляют предварительный убыток.
        super::daily::load(&core).await;
        // Решение 416 (ревью С-3): монеты бирж по сетям и сеть по умолчанию — до сделок (ключ счёта не меняется).
        super::coin_account::load(&core).await;
        super::ledger::load(&core).await;
        // Решение 163: возвраты USDT «ждёт повтора» продолжают повторы после перезапуска.
        super::usdt_return_retry::resume(&core);
        super::gas_stats::load(&core).await;
        super::manual::load(&core).await;
        // Решение 408 (ответ Тиграна 9а): ожидания ручных кнопок прошлого запуска — «потеряно при перезапуске».
        super::manual_wait_mark::lost(&core).await;
        super::transit::load(&core).await;
        super::withdraw_seen::load(&core).await;
        // Решение 413: замеры удержания бирж при зачислении (deposit_hold.rs).
        super::deposit_hold::load(&core).await;
        super::low_usdt::load(&core).await;
        // Задача эпизодов (решение 74): при запуске закрывает прерванные перезапуском, дальше — запись и уборка.
        if let Some(rx) = lk(&core.inner.episode_rx).take() {
            tokio::spawn(super::episode_worker::run(core.clone(), rx));
        }
        // Задача окон (решение 125): при запуске закрывает прерванные перезапуском, дальше — запись, закрытие, уборка.
        if let Some(rx) = lk(&core.inner.window_rx).take() {
            tokio::spawn(super::window_worker::run(core.clone(), rx));
        }
        // Решение 245: точки цены DEX на объём открытой сделки — своя задача (тик только ставит отметку).
        tokio::spawn(super::window_deal_run::run(core.clone()));
        // Решение 317 (ревью 16.11): признак отметок изменения стакана — до приёма событий; дальше — по изменениям настроек.
        super::book_marks_on::refresh(&core);
        tokio::spawn(super::book_marks_on::watch(core.clone()));
        tokio::spawn(super::tick::run(core.clone(), market_rx));
        tokio::spawn(super::rebuild::run(core.clone()));
        tokio::spawn(super::candidates::run(core.clone(), cand_rx));
        tokio::spawn(super::reconcile::run(core.clone()));
        // Решение 225: софтовые паузы снимает сам бот — проверкой подключения и с новым днём (UTC).
        tokio::spawn(super::auto_resume::run(core.clone()));
        tokio::spawn(super::day_roll::run(core.clone()));
        // Решение 414: слепки баланса направлений для лимита просадки — при запуске и при задании risk.drawdown_*.
        tokio::spawn(super::drawdown_ask::run(core.clone()));
        tokio::spawn(super::rebuild::watch_settings(core.clone()));
        // Решение 348: сводка «кто вызвал котировку» (log.quote_count_s; не задана — ждёт настройку).
        tokio::spawn(super::quote_count_log::run(core.clone()));
        // Решение 349 (ревью Б2): рассылка пробуждений промежуточным пулом — своя задача, поток тика только отмечает.
        tokio::spawn(super::mid_wake::run(core.clone()));
        // Решение 124: исполнение циклов DEX ↔ DEX — одна задача, толчки событий пулов сливаются.
        tokio::spawn(super::cycle_watch::run(core.clone()));
        // Решение 316: прогрев адресов депозита бирж (venue.deposit_warm) — первая сделка после перезапуска без запроса.
        tokio::spawn(super::deposit_warm::run(core.clone()));
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
    /// Решение 316: и толчок прогрева адресов депозита с повтором отказавших монет (deposit_warm.rs).
    pub fn secrets_changed(&self) {
        super::net_kick::kick(self);
        // Решение 411 (триггер б): память адресов прежнего ключа по новому отпечатку не находится (отпечаток сменился
        // вместе с ключом в хранилище); прогрев проверяет память заново.
        super::deposit_warm::kick_account();
        // Решение 387: запросы минимума депозита по требованию расчёта — заново (deposit_min_ask.rs).
        super::deposit_min_ask::forget();
        self.request_rebuild();
    }
}
