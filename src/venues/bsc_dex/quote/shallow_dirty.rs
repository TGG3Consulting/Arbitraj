//! Файл: venues/bsc_dex/quote/shallow_dirty.rs
//! Что делает: решение 403 (ревью t188, Н1) — пары «монета — встречный токен», чьи пулы записаны в реестр после
//! прошлого прохода построителя отбора мелких пулов (shallow_build.rs), и признак «нужен полный проход» (retain —
//! пулы удалены). Построитель берёт набор целиком (take) и считает только эти пары (и пары, чей встречный токен —
//! монета изменённой пары «встречный — USDT»), без полного снимка реестра. Отметки ставит реестр после своей записи,
//! только когда предел задан (shallow_memo.rs on); путь котировки набор не трогает.
//! Кто вызывает: quote/shallow_memo.rs (wrote, wrote_all, take_dirty).
//! От чего зависит: quote/shallow_stamps.rs (Pair).
//! Настройки: нет.

use std::collections::HashSet;
use std::sync::Mutex;

use super::shallow_stamps::Pair;

/// Набор под замком.
#[derive(Default)]
struct Inner {
    pairs: HashSet<Pair>,
    full: bool,
}

/// Изменённые пары. Общий у клонов памяти (за `Arc` памяти).
#[derive(Default)]
pub struct Dirty(Mutex<Inner>);

impl Dirty {
    /// Отметить пары. Ошибок нет.
    pub fn mark(&self, pairs: &[Pair]) {
        let mut m = self.0.lock().unwrap_or_else(|p| p.into_inner());
        m.pairs.extend(pairs.iter().copied());
    }

    /// Нужен полный проход. Ошибок нет.
    pub fn mark_full(&self) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).full = true;
    }

    /// Забрать набор: (пары, нужен ли полный проход); набор пуст после вызова. Ошибок нет.
    pub fn take(&self) -> (HashSet<Pair>, bool) {
        let mut m = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let full = std::mem::take(&mut m.full);
        (std::mem::take(&mut m.pairs), full)
    }
}
