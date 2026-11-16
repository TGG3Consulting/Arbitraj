//! Файл: venues/bsc_dex/pools/pool_unread.rs
//! Что делает: пулы, которые ни разу не прочитались (решение 339, экран «Монеты»): у такого пула нет записи в реестре
//! (запись появляется только после удачного чтения), поэтому экран, построенный по реестру, его не видел. Здесь —
//! что нужно экрану: монета пула, второй токен, вид и последняя причина неудачи. Отметка ставится при неудаче чтения
//! пула без записи в реестре (приём пачки admit.rs, повтор reload.rs) — для всех видов пулов, без настройки;
//! снимается при любой записи пула в реестр (registry.rs put) и когда пула нет среди найденных (market.rs keep_only).
//! Ядро и расчёты эти пулы не видят: это только строки экрана (depth_facts.rs).
//! Кто вызывает: admit.rs, reload.rs (note), registry.rs (хранит, clear при put), market.rs (keep_only),
//! depth_facts.rs (list).
//! От чего зависит: discover.rs (Found), families.rs (Family).
//! Настройки: нет.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use alloy_primitives::Address;

use crate::core::types::PoolRef;

use super::discover::Found;
use super::families::Family;

/// Пул, ни разу не прочитанный: кто он и почему не прочитан.
#[derive(Debug, Clone)]
pub struct Unread {
    /// Вид пула (DEX).
    pub family: &'static Family,
    /// Монета пула (не встречный токен).
    pub coin: Address,
    /// Второй токен пула (USDT или промежуточная монета).
    pub other: Address,
    /// Комиссия пула из справочника (V3 — fee события, V4 — PoolKey.fee); None — не из пула.
    pub fee_pips: Option<u32>,
    /// Последняя причина неудачи чтения (текст ошибки).
    pub why: String,
}

/// Непрочитанные пулы площадки. Дешёво клонируется (хранится в реестре).
#[derive(Clone, Default)]
pub struct Book {
    map: Arc<Mutex<HashMap<PoolRef, Unread>>>,
    /// Сколько отметок — без отметок clear не берёт замок (put реестра — горячий путь).
    marks: Arc<AtomicUsize>,
}

impl Book {
    /// Короткий замок карты. Ошибок нет.
    fn lock(&self) -> MutexGuard<'_, HashMap<PoolRef, Unread>> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Счётчик отметок — после изменения карты (под её замком). Ошибок нет.
    fn sync(&self, m: &HashMap<PoolRef, Unread>) {
        self.marks.store(m.len(), Ordering::Release);
    }

    /// Что делает: отметка «пул не прочитан» (новая или с новой причиной). Принимает: найденный пул, причину.
    /// Ошибок нет.
    pub fn note(&self, f: &Found, why: &str) {
        let other = if f.token0 == f.coin {
            f.token1
        } else {
            f.token0
        };
        let u = Unread {
            family: f.family,
            coin: f.coin,
            other,
            fee_pips: f.fee_pips,
            why: why.to_string(),
        };
        let mut m = self.lock();
        m.insert(f.reference.clone(), u);
        self.sync(&m);
    }

    /// Что делает: снимает отметку (пул записан в реестр); отметок нет — без замка. Ошибок нет.
    pub fn clear(&self, r: &PoolRef) {
        if self.marks.load(Ordering::Acquire) == 0 {
            return;
        }
        let mut m = self.lock();
        if m.remove(r).is_some() {
            self.sync(&m);
        }
    }

    /// Что делает: оставляет отметки только пулов `known` (найденные подпиской). Ошибок нет.
    pub fn keep_only(&self, known: &HashSet<PoolRef>) {
        if self.marks.load(Ordering::Acquire) == 0 {
            return;
        }
        let mut m = self.lock();
        m.retain(|r, _| known.contains(r));
        self.sync(&m);
    }

    /// Что делает: копия всех отметок (для экрана). Ошибок нет.
    pub fn list(&self) -> Vec<(PoolRef, Unread)> {
        if self.marks.load(Ordering::Acquire) == 0 {
            return Vec::new();
        }
        self.lock()
            .iter()
            .map(|(r, u)| (r.clone(), u.clone()))
            .collect()
    }
}
