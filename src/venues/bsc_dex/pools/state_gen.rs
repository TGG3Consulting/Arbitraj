//! Файл: venues/bsc_dex/pools/state_gen.rs
//! Что делает: «поколение состояния» записи реестра пулов (решение 301, часть е2; задача t132). Поколение (Entry.state_gen)
//! растёт только при ФАКТИЧЕСКОЙ смене того, от чего зависит глубина пула и курс через него: состояние пула
//! (pool.state, PoolState: PartialEq), карта тиков (сначала Arc::ptr_eq, иначе сравнение содержимого), бины (так же),
//! достоверность (valid) и комиссия пула (pool.fee_pips — через неё курс встречного токена к USDT, quote/rate.rs).
//! Ключ глубины (depth_key.rs) и отметки пар (registry.rs pair_marks) берут поколение вместо (synced_block, synced_log):
//! пул, перечитанный на новом блоке без изменения состояния, не заставляет пересчитывать глубину тысяч пулов со
//! встречным токеном не USDT (например WBNB). synced_block / synced_log не трогаются — ими пользуются проверки свежести
//! (has_applied, verify.rs, check/quoter_block.rs).
//! Поколение 0 — «вне реестра» (запись собрана или изменена в обход put/update, например load::renewed — свежее
//! состояние предторговой сверки): ключ глубины с поколением 0 не совпадает ни с каким (depth_key.rs), глубина такой
//! записи всегда считается заново, как раньше при новом (synced_block, synced_log).
//! Кто вызывает: pools/registry.rs (put, update).
//! От чего зависит: registry.rs (Entry), ticks.rs, bins.rs, core/types (PoolState).
//! Настройки: нет.

use std::sync::Arc;

use crate::core::types::PoolState;

use super::bins::BinMap;
use super::registry::Entry;
use super::ticks::TickMap;

/// Снимок того, что входит в поколение (до изменения записи в update). Копия дешёвая: тики и бины — за Arc.
pub struct Snap {
    state: PoolState,
    fee: Option<u32>,
    ticks: Option<TickMap>,
    bins: Option<BinMap>,
    valid: bool,
}

impl Snap {
    /// Снимок записи. Ошибок нет.
    pub fn of(e: &Entry) -> Snap {
        Snap {
            state: e.pool.state.clone(),
            fee: e.pool.fee_pips,
            ticks: e.ticks.clone(),
            bins: e.bins.clone(),
            valid: e.valid,
        }
    }

    /// Та же ли запись по составу поколения. Ошибок нет.
    pub fn same(&self, e: &Entry) -> bool {
        self.valid == e.valid
            && self.fee == e.pool.fee_pips
            && self.state == e.pool.state
            && same_ticks(self.ticks.as_ref(), e.ticks.as_ref())
            && same_bins(self.bins.as_ref(), e.bins.as_ref())
    }
}

/// Та же ли запись `b`, что `a`, по составу поколения (put: старая и новая запись). Ошибок нет.
pub fn same(a: &Entry, b: &Entry) -> bool {
    a.valid == b.valid
        && a.pool.fee_pips == b.pool.fee_pips
        && a.pool.state == b.pool.state
        && same_ticks(a.ticks.as_ref(), b.ticks.as_ref())
        && same_bins(a.bins.as_ref(), b.bins.as_ref())
}

/// Карты тиков равны: шаг, прочитанные слова и тики (сначала по указателю Arc). Ошибок нет.
fn same_ticks(a: Option<&TickMap>, b: Option<&TickMap>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            x.spacing == y.spacing
                && x.words == y.words
                && (Arc::ptr_eq(&x.ticks, &y.ticks) || x.ticks == y.ticks)
        }
        _ => false,
    }
}

/// Бины равны: шаг, прочитанная полоса и резервы (сначала по указателю Arc). Ошибок нет.
fn same_bins(a: Option<&BinMap>, b: Option<&BinMap>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            x.step == y.step
                && x.range == y.range
                && (Arc::ptr_eq(&x.reserves, &y.reserves) || x.reserves == y.reserves)
        }
        _ => false,
    }
}
