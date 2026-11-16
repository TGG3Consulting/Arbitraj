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
//! Кто вызывает: gateway/core_out_side.rs (venue.book_events_keep, dex.pool_events_keep, log.pool_events_lost_interval_s).
//! От чего зависит: settings/store.rs (resolve, subscribe), tokio (задача, broadcast).
//! Настроек нет (имя настройки передаёт вызывающий).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;

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

/// Ячейка настройки с перечитыванием по изменениям (см. шапку). Принимает: хранилище, имя, площадку области (None —
/// только вся система), разбор. Возвращает: ячейку. Ошибок нет.
pub fn watch(store: &Store, name: &'static str, venue: Option<String>, parse: Parse) -> Arc<Cell> {
    let cell = Arc::new(Cell::default());
    // Подписка — до чтения: изменение между чтением и подпиской не теряется.
    let mut rx = store.subscribe();
    cell.put(read(store, name, venue.as_deref(), parse));
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
                cell.put(read(&store, name, venue.as_deref(), parse));
            }
        }
    });
    cell
}

/// Действующее значение (Store::resolve: площадка сильнее всей системы), без текста ошибки. Ошибок нет.
fn read(store: &Store, name: &str, venue: Option<&str>, parse: Parse) -> Option<u64> {
    store.resolve(name, venue, None).as_ref().and_then(parse)
}
