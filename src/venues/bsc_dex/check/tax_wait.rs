//! Файл: venues/bsc_dex/check/tax_wait.rs
//! Что делает: решения цикла налогов (check/tax_cycle.rs) без ввода-вывода (решение 63):
//! - кого из ждущих проверить сейчас: по сигналу «пул готов» — ждущих пула, у которых появился достоверный, и ждущих
//!   своих пулов (Why::Loading), у которых пулы дочитаны или брошены; без сигнала — только последних (сигнал мог
//!   прийти, пока шла проверка);
//! - сводка первого прохода: когда у всех монет, КРОМЕ ждущих своих пулов, была первая проверка — одна сводка;
//!   «пулы ещё загружаются: N» — отдельным числом (монета с вечно недочитанными пулами сводку не держит).
//!
//! Кто вызывает: check/tax_cycle.rs, tests/bsc_dex_tax_wait.rs.
//! От чего зависит: check/tax_pass.rs (Pass, Res, Why).
//! Настройки: нет.

use std::collections::HashSet;

use alloy_primitives::Address;

use super::tax_pass::{Pass, Res, Why};

/// Ждущая монета: адрес, тикер, почему ждёт.
pub type Waiting = (Address, String, Why);

/// Кого проверить сейчас. Принимает: ждущих, был ли сигнал «пул готов», сколько пулов монеты ещё грузится, есть ли у
/// монеты достоверный пул. Возвращает: (адрес, тикер) по порядку ждущих. Ошибок нет.
pub fn due(
    waiting: &[Waiting],
    signal: bool,
    pending: impl Fn(Address) -> usize,
    has_valid: impl Fn(&str) -> bool,
) -> Vec<(Address, String)> {
    waiting
        .iter()
        .filter(|(a, t, w)| match w {
            Why::Loading => pending(*a) == 0,
            Why::NoPool => signal && has_valid(t),
            _ => false,
        })
        .map(|(a, t, _)| (*a, t.clone()))
        .collect()
}

/// Сводка первого прохода.
#[derive(Debug, Default)]
pub struct First {
    /// Монеты без первой проверки.
    pub left: HashSet<Address>,
    /// Из них — ждут своих пулов (последнее, что о них известно).
    pub loading: HashSet<Address>,
    /// [прошли, не прошли, ждут, не задана настройка] по первой проверке.
    pub counts: [usize; 4],
    pub sent: bool,
}

impl First {
    /// Новая сводка: все монеты без первой проверки. Ошибок нет.
    pub fn new(coins: impl IntoIterator<Item = Address>) -> Self {
        Self {
            left: coins.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Учитывает проход. Возвращает: счётчики сводки [прошли, не прошли, ждут, не задана настройка, пулы ещё
    /// загружаются], если её пора отправить (один раз), иначе None. Ошибок нет.
    pub fn tally(&mut self, pass: &Pass) -> Option<[usize; 5]> {
        for (a, _, w) in &pass.waiting {
            if *w == Why::Loading && self.left.contains(a) {
                self.loading.insert(*a);
            }
        }
        for (a, r) in &pass.done {
            self.loading.remove(a);
            if self.left.remove(a) {
                let k = match r {
                    Res::Ok => 0,
                    Res::Failed => 1,
                    Res::Wait => 2,
                    Res::Config => 3,
                };
                self.counts[k] += 1;
            }
        }
        if self.sent || !self.left.iter().all(|a| self.loading.contains(a)) {
            return None;
        }
        self.sent = true;
        let [ok, failed, waiting, config] = self.counts;
        Some([ok, failed, waiting, config, self.left.len()])
    }
}
