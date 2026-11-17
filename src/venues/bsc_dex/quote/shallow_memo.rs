//! Файл: venues/bsc_dex/quote/shallow_memo.rs
//! Что делает: решение 393 (повторное ревью t178, Н-B2, Н-B1) — память отбора мелких пулов пары (quote/shallow_pick.rs):
//! - отбор пары считается один раз на версию реестра и набор настроек (сборка графа, проход пробы и сверки берут
//!   готовый; раньше admitted считал ранги всей пары на каждый пул — k² расчётов на пару);
//! - прошлый отбор пары помнится через смену версий: при равном ранге уже отобранный пул остаётся (гистерезис на
//!   состоянии, без числа-порога), его вытесняет только претендент со строго лучшим рангом.
//!
//! Хранится в реестре (pools/registry.rs поле shallow, Arc — общий у клонов реестра); у замороженного снимка — свой
//! пустой.
//! Кто вызывает: quote/shallow_pick.rs.
//! От чего зависит: pools/registry.rs (версия), core/types (PoolRef).
//! Настройки: нет (набор настроек — ключ, его даёт вызывающий).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use alloy_primitives::Address;

use crate::core::types::PoolRef;

/// Пара «монета — встречный токен».
pub type Pair = (Address, Address);

/// Состояние памяти.
#[derive(Default)]
struct Inner {
    /// Ключ действующих отборов: (версия реестра, настройки текстом).
    key: Option<(u64, Vec<String>)>,
    /// Отбор пары для ключа.
    now: HashMap<Pair, Arc<Vec<PoolRef>>>,
    /// Последний отбор пары (любой ключ) — для гистерезиса.
    last: HashMap<Pair, Arc<Vec<PoolRef>>>,
}

/// Память отбора. Дешёво клонируется (общая у клонов реестра).
#[derive(Clone, Default)]
pub struct Memo(Arc<Mutex<Inner>>);

impl Memo {
    /// Отбор пары для ключа (версия, настройки): готовый или посчитанный `calc` (ему передаётся прошлый отбор пары).
    /// Ошибок нет.
    pub fn get(
        &self,
        key: (u64, Vec<String>),
        pair: Pair,
        calc: impl FnOnce(&[PoolRef]) -> Vec<PoolRef>,
    ) -> Arc<Vec<PoolRef>> {
        let prev = {
            let mut g = self.0.lock().unwrap_or_else(|p| p.into_inner());
            if g.key.as_ref() != Some(&key) {
                g.key = Some(key.clone());
                g.now.clear();
            }
            if let Some(v) = g.now.get(&pair) {
                return Arc::clone(v);
            }
            g.last.get(&pair).cloned().unwrap_or_default()
        };
        // Расчёт — без замка (он читает реестр и настройки).
        let v = Arc::new(calc(&prev));
        let mut g = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if g.key.as_ref() == Some(&key) {
            g.now.insert(pair, Arc::clone(&v));
        }
        g.last.insert(pair, Arc::clone(&v));
        v
    }
}
