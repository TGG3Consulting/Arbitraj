//! Файл: gateway/setting_cell.rs
//! Что делает: ревью 324 (Б2) — значение одной настройки в памяти для горячего пути (каждый кадр стакана, каждое событие
//! пула): чтение — одна атомарная загрузка, без Store::resolve (выделения памяти, RwLock хранилища) и без замков.
//! Значение читается при создании ячейки и перечитывается по рассылке изменений хранилища (Store::subscribe — как у
//! других подписчиков, settings/live.rs) отдельной задачей tokio: пришло изменение настройки с этим именем (любая
//! область) или подписчик отстал (изменения могли пропасть) — Store::resolve заново. Подписка берётся ДО первого
//! чтения — изменение между ними не теряется. Новое значение действует, как только задача обработает изменение
//! (без перезапуска). Задача кончается, когда ячейка никому не нужна (проверяется при следующем изменении) или рассылка
//! закрыта. Нет рантайма tokio при создании — задачи нет, значение остаётся прочитанным при создании (адаптеры и тесты
//! создают ячейки внутри рантайма).
//! Хранение: AtomicU64, 0 — «не задана / не того вида», n + 1 — значение n (u64::MAX хранится как u64::MAX − 1 —
//! насыщение; для секунд интервала и признака да/нет разницы нет).
//! Ревью 325 (Н1): watch_in — то же для ровно одной области (Store::get_exact; область «DEX» — настройки по одному DEX,
//! например dex.event_refresh_batch / dex.event_refresh_fee_only, pools/event_flags.rs).
//! Кто вызывает: gateway/core_out_side.rs (venue.book_events_keep, dex.pool_events_keep, log.pool_events_lost_interval_s);
//! venues/bsc_dex/pools/event_flags.rs (watch_in).
//! От чего зависит: settings/store.rs (resolve, get_exact, subscribe), settings/scope.rs (Scope), tokio (задача,
//! broadcast).
//! Настроек нет (имя настройки передаёт вызывающий).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;

use crate::settings::scope::Scope;
use crate::settings::store::Store;

/// Разбор значения настройки в число ячейки (None — не того вида: как «не задана»).
pub type Parse = fn(&Value) -> Option<u64>;

/// Да/нет: да — 1, нет — 0. Ошибок нет.
pub fn bool_of(v: &Value) -> Option<u64> {
    v.as_bool().map(u64::from)
}

/// Целое без знака как есть (секунды интервала). Ошибок нет.
pub fn u64_of(v: &Value) -> Option<u64> {
    v.as_u64()
}

/// Значение настройки в памяти (см. шапку).
#[derive(Debug, Default)]
pub struct Cell(AtomicU64);

impl Cell {
    /// Значение: None — не задана (или не того вида). Одна атомарная загрузка. Ошибок нет.
    pub fn get(&self) -> Option<u64> {
        self.0.load(Ordering::Relaxed).checked_sub(1)
    }

    /// Признак да/нет: true — только «да» (не задана / нет — false). Ошибок нет.
    pub fn on(&self) -> bool {
        self.get() == Some(1)
    }

    /// Записывает значение. Ошибок нет.
    fn put(&self, v: Option<u64>) {
        self.0
            .store(v.map_or(0, |n| n.saturating_add(1)), Ordering::Relaxed);
    }
}

/// Где читается значение ячейки.
enum At {
    /// Store::resolve: площадка (если задана) сильнее всей системы.
    Resolve(Option<String>),
    /// Store::get_exact: ровно эта область (ревью 325 — область «DEX»).
    Exact(Scope),
}

/// Ячейка настройки с перечитыванием по изменениям (см. шапку). Принимает: хранилище, имя, площадку области (None —
/// только вся система), разбор. Возвращает: ячейку. Ошибок нет.
pub fn watch(store: &Store, name: &'static str, venue: Option<String>, parse: Parse) -> Arc<Cell> {
    start(store, name, At::Resolve(venue), parse)
}

/// Ревью 325: ячейка настройки ровно одной области (без подстановки других областей) с перечитыванием по изменениям.
/// Принимает: хранилище, имя, область, разбор. Возвращает: ячейку. Ошибок нет.
pub fn watch_in(store: &Store, name: &'static str, scope: Scope, parse: Parse) -> Arc<Cell> {
    start(store, name, At::Exact(scope), parse)
}

/// Общее для watch / watch_in: подписка, первое чтение, задача перечитывания. Ошибок нет.
fn start(store: &Store, name: &'static str, at: At, parse: Parse) -> Arc<Cell> {
    let cell = Arc::new(Cell::default());
    // Подписка — до чтения: изменение между чтением и подпиской не теряется.
    let mut rx = store.subscribe();
    cell.put(read(store, name, &at, parse));
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return cell;
    };
    let (store, weak) = (store.clone(), Arc::downgrade(&cell));
    rt.spawn(async move {
        loop {
            let again = match rx.recv().await {
                Ok(c) => c.name == name,
                Err(RecvError::Lagged(_)) => true,
                Err(RecvError::Closed) => return,
            };
            let Some(cell) = weak.upgrade() else {
                return;
            };
            if again {
                cell.put(read(&store, name, &at, parse));
            }
        }
    });
    cell
}

/// Действующее значение (Resolve: площадка сильнее всей системы; Exact: ровно область), без текста ошибки. Ошибок нет.
fn read(store: &Store, name: &str, at: &At, parse: Parse) -> Option<u64> {
    let v = match at {
        At::Resolve(venue) => store.resolve(name, venue.as_deref(), None),
        At::Exact(scope) => store.get_exact(name, scope),
    };
    v.as_ref().and_then(parse)
}
