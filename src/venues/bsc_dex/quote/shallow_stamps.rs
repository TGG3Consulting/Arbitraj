//! Файл: venues/bsc_dex/quote/shallow_stamps.rs
//! Что делает: решение 403 (ревью t188, Н1) — отпечатки входов отбора мелких пулов по парам для путей ВНЕ котировки
//! (shallow_pick.rs pick: подписка, сверка с квотером, проба налогов). Построитель (shallow_build.rs) меняет только
//! слоты пересчитанных пар; путь котировки этот набор не читает (у него — снимок допуска, shallow_memo.rs).
//! Кто вызывает: quote/shallow_build.rs (apply, clear), quote/shallow_pick.rs (get), quote/shallow_memo.rs (хранит).
//! От чего зависит: core/types (PoolRef), alloy_primitives.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use alloy_primitives::Address;

use crate::core::types::PoolRef;

/// Пара «монета — встречный токен».
pub type Pair = (Address, Address);

/// Отпечаток входов отбора пары: всё, от чего он зависит (shallow_pick.rs calc). Одинаковый — отбор тот же.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stamp {
    /// Поколение значений настроек (settings/store.rs generation) — в нём и предел, и настройки глубины и пробы.
    pub settings: u64,
    /// Адрес USDT (venue.quote_asset) и его знаки.
    pub usdt: Address,
    pub dec: u8,
    /// (поколение состояния, спит) пулов пары, по возрастанию (registry.rs pair_stamp).
    pub pools: Vec<(u64, bool)>,
    /// То же у пулов «встречный — USDT» (курс суммы пробы, quote/rate.rs); у пары с USDT — пусто.
    pub rate: Vec<(u64, bool)>,
}

/// Отбор пары и отпечаток входов, на которых он посчитан.
#[derive(Debug, Clone, Default)]
pub struct Slot {
    pub stamp: Stamp,
    pub sel: Arc<Vec<PoolRef>>,
}

/// Отпечатки по парам. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Stamps(Arc<RwLock<HashMap<Pair, Arc<Slot>>>>);

impl Stamps {
    /// Слот пары. None — пара не считалась. Ошибок нет.
    pub fn get(&self, pair: &Pair) -> Option<Arc<Slot>> {
        self.0
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(pair)
            .cloned()
    }

    /// Замена слотов пересчитанных пар (None — пара ушла из реестра). Пусто — замок не берётся. Ошибок нет.
    pub fn apply(&self, changed: Vec<(Pair, Option<Arc<Slot>>)>) {
        if changed.is_empty() {
            return;
        }
        let mut m = self.0.write().unwrap_or_else(|p| p.into_inner());
        for (p, s) in changed {
            match s {
                Some(s) => {
                    m.insert(p, s);
                }
                None => {
                    m.remove(&p);
                }
            }
        }
    }

    /// Все слоты долой (предел снят, USDT не известен). Ошибок нет.
    pub fn clear(&self) {
        self.0.write().unwrap_or_else(|p| p.into_inner()).clear();
    }
}
