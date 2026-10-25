//! Файл: venues/bsc_dex/check/tax_wait.rs
//! Что делает: решения цикла налогов (check/tax_cycle.rs) без ввода-вывода (решение 63):
//! - кого из ждущих проверить сейчас: по сигналу «пул готов» — ждущих пула, у которых появился достоверный, и ждущих
//!   своих пулов (Why::Loading), у которых пулы дочитаны или брошены; без сигнала — только последних (сигнал мог
//!   прийти, пока шла проверка);
//! - сводка первого прохода: когда у всех монет, кроме тех, чьи пулы застряли (ждут незаданную настройку), была первая
//!   проверка — одна сводка; «пулы застряли: N» — отдельным числом. Монеты с реально читающимися пулами сводку держат.
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

    /// Учитывает проход. Сводку держат монеты без первой проверки, КРОМЕ тех, чьи пулы застряли (все их ждущие пулы
    /// ждут незаданную настройку — pools/loading.rs stuck_only): монеты с реально читающимися пулами держат.
    /// Принимает: проход, «застряли ли пулы монеты». Возвращает: счётчики сводки [прошли, не прошли, ждут, не задана
    /// настройка, пулы застряли], если её пора отправить (один раз), иначе None. Ошибок нет.
    pub fn tally(&mut self, pass: &Pass, stuck: impl Fn(Address) -> bool) -> Option<[usize; 5]> {
        for (a, r) in &pass.done {
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
        if self.sent || !self.left.iter().all(|a| stuck(*a)) {
            return None;
        }
        self.sent = true;
        let [ok, failed, waiting, config] = self.counts;
        Some([ok, failed, waiting, config, self.left.len()])
    }
}
