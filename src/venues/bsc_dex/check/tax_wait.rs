//! Файл: venues/bsc_dex/check/tax_wait.rs
//! Что делает: решения цикла налогов (check/tax_cycle.rs) без ввода-вывода (решение 63):
//! - кого из ждущих проверить сейчас: по сигналу «пул готов» — ждущих
//!   глубокого пула (Why::NoPool, Why::Shallow — решение 73), у которых он появился, и ждущих
//!   своих пулов (Why::Loading), у которых пулы дочитаны или брошены; без сигнала — только последних (сигнал мог
//!   прийти, пока шла проверка); на каждом шаге цикла (due_now) — то же, что по сигналу: сигнал «пул готов», пришедший,
//!   пока цикл проверял другие монеты, теряется (notify_waiters), а пулы от поиска по фабрикам приходят позже первого
//!   прохода (живой бот 28.11.2026, base_dex);
//! - кого проверить при изменении настроек: ждущих «не задана настройка» (due_config);
//! - сводка первого прохода: когда у всех монет, кроме тех, чьи пулы застряли (ждут незаданную настройку), была первая
//!   проверка — одна сводка; «пулы застряли: N» — отдельным числом. Монеты с реально читающимися пулами сводку держат.
//!
//! Кто вызывает: check/tax_cycle.rs, tests/bsc_dex_tax_wait.rs.
//! От чего зависит: check/tax_pass.rs (Pass, Res, Why).
//! Настройки: нет.

use std::collections::{HashMap, HashSet};

use alloy_primitives::Address;

use super::tax_pass::{Pass, Res, Why};

/// Ждущая монета: адрес, тикер, почему ждёт.
pub type Waiting = (Address, String, Why);

/// Кого проверить сейчас. Принимает: ждущих, был ли сигнал «пул готов», сколько пулов монеты ещё грузится, есть ли у
/// монеты ГЛУБОКИЙ пул (решение 73: достоверный, но мелкий пробу не даст). Возвращает: (адрес, тикер) по порядку
/// ждущих. Ошибок нет.
pub fn due(
    waiting: &[Waiting],
    signal: bool,
    pending: impl Fn(Address) -> usize,
    has_deep: impl Fn(Address) -> bool,
) -> Vec<(Address, String)> {
    waiting
        .iter()
        .filter(|(a, _, w)| match w {
            Why::Loading => pending(*a) == 0,
            Why::NoPool | Why::Shallow => signal && has_deep(*a),
            _ => false,
        })
        .map(|(a, t, _)| (*a, t.clone()))
        .collect()
}

/// Кого проверить на каждом шаге цикла налогов, до ожидания сигнала (check/tax_cycle.rs): то же, что по сигналу «пул
/// готов» (`due` с signal = true) — ждущие своих пулов, у которых они дочитаны, и ждущие глубокого пула (Why::NoPool,
/// Why::Shallow), у которых он уже есть.
/// Почему не только по сигналу (живой бот 28.11.2026, base_dex, сборка e9907a2f): «пул готов» — Notify::notify_waiters
/// (pools/progress.rs), он будит только того, кто ждёт в эту минуту, разрешение не хранится. Пулы, найденные поиском по
/// фабрикам после первого прохода (3025 пулов, «Новый пул» ×2946), дочитывались, пока цикл проверял монеты по прошлому
/// сигналу; их сигналы терялись, а шаг цикла раньше брал без сигнала только Why::Loading — WETH и cbBTC (промежуточные,
/// Why::NoPool с первого прохода) 20 минут оставались «налоги промежуточной монеты не проверены» до ручного
/// «Перепроверить». Состояние реестра смотрится здесь на каждом шаге — пропущенный сигнал больше ничего не теряет.
/// Монета, только что получившая Why::NoPool / Why::Shallow, глубокого пула в тот же миг не имеет (tax_pass.rs: та же
/// проверка has_valid / deep), поэтому лишних кругов нет. Ошибок нет.
pub fn due_now(
    waiting: &[Waiting],
    pending: impl Fn(Address) -> usize,
    has_deep: impl Fn(Address) -> bool,
) -> Vec<(Address, String)> {
    due(waiting, true, pending, has_deep)
}

/// Кого проверить, когда отметка «данные пулов отстают» (решение 72) снята (`busy` — стоит ли она сейчас): всех
/// ждущих Why::Lag. Проверяется на каждом шаге цикла, а не только по сигналу — снятие до постановки в ожидание не
/// теряется. Ошибок нет.
pub fn due_lag(waiting: &[Waiting], busy: bool) -> Vec<(Address, String)> {
    if busy {
        return Vec::new();
    }
    waiting
        .iter()
        .filter(|(_, _, w)| *w == Why::Lag)
        .map(|(a, t, _)| (*a, t.clone()))
        .collect()
}

/// Кого проверить при изменении настроек `changed` (имена, пачка схлопнута — settings_wait::drain): ждущих «не задана
/// настройка» (Why::Config), чья незаданная настройка (`names` — по монете, имена из ошибки хранилища) среди изменённых
/// — сразу, а не через полный проход tax.recheck_interval_s. Посторонние изменения перепроверок не вызывают (ревью #65:
/// иначе 30 изменений подряд — 30 проходов по всем ждущим). Монета без имени настройки (ключ кошелька) — не по
/// изменениям настроек. Ошибок нет.
pub fn due_config(
    waiting: &[Waiting],
    names: &HashMap<Address, Vec<String>>,
    changed: &HashSet<String>,
) -> Vec<(Address, String)> {
    waiting
        .iter()
        .filter(|(a, _, w)| {
            *w == Why::Config
                && names
                    .get(a)
                    .is_some_and(|n| n.iter().any(|x| changed.contains(x)))
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
