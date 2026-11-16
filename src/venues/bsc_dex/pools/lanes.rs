//! Файл: venues/bsc_dex/pools/lanes.rs
//! Что делает: решение 330 — расписание «полос» событий пулов сессии пачки (чистая логика, без узла и без задач): у
//! каждого пула (ключ) своя полоса — его события идут строго по порядку прихода и по одному (следующее — только после
//! завершения предыдущего), а полосы РАЗНЫХ пулов идут одновременно, не больше `max` сразу. Пулы, ждущие свободного
//! места, запускаются в порядке, в каком у них появилась работа. Само выполнение (задачи tokio, каналы логов) —
//! pools/session_lanes.rs; здесь — только счёт: сколько ждёт у ключа, кто выполняется, кто следующий.
//! Кто вызывает: pools/session_lanes.rs; тесты (tests/bsc_dex_session_lanes_330.rs).
//! От чего зависит: std.
//! Настройки: нет (предел max передаёт вызывающий — dex.event_lanes_max).

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;

/// Расписание полос (см. шапку).
#[derive(Debug)]
pub struct Lanes<K> {
    /// Сколько работ ждёт запуска у ключа (только ключи с ненулевым числом).
    waiting: HashMap<K, usize>,
    /// Ключи, у которых работа выполняется сейчас (у ключа — не больше одной).
    running: HashSet<K>,
    /// Ключи, готовые к запуску (есть ждущая работа и ничего не выполняется), по порядку готовности.
    ready: VecDeque<K>,
}

impl<K: Eq + Hash + Copy> Default for Lanes<K> {
    fn default() -> Self {
        Lanes {
            waiting: HashMap::new(),
            running: HashSet::new(),
            ready: VecDeque::new(),
        }
    }
}

impl<K: Eq + Hash + Copy> Lanes<K> {
    /// Что делает: пустое расписание. Ошибок нет.
    pub fn new() -> Self {
        Self::default()
    }

    /// Что делает: у ключа `k` появилась работа (в конец его полосы). Ключ без работы и не выполняющийся становится
    /// готовым (в конец очереди готовых). Принимает: ключ. Ошибок нет.
    pub fn push(&mut self, k: K) {
        let n = self.waiting.entry(k).or_insert(0);
        *n += 1;
        if *n == 1 && !self.running.contains(&k) {
            self.ready.push_back(k);
        }
    }

    /// Что делает: следующий ключ к запуску при пределе `max` одновременных полос. Ключ считается выполняющимся, его
    /// ждущих становится на одну меньше. Принимает: предел (0 — как 1: хотя бы одна полоса идёт, иначе работа
    /// встала бы навсегда). Возвращает: ключ; None — предел занят или готовых нет. Ошибок нет.
    pub fn next(&mut self, max: usize) -> Option<K> {
        if self.running.len() >= max.max(1) {
            return None;
        }
        let k = self.ready.pop_front()?;
        let left = self.waiting.get(&k).copied().unwrap_or(0);
        if left > 1 {
            self.waiting.insert(k, left - 1);
        } else {
            self.waiting.remove(&k);
        }
        self.running.insert(k);
        Some(k)
    }

    /// Что делает: работа ключа `k` завершена. Есть ещё ждущие у ключа — он снова готов (в конец очереди готовых).
    /// Принимает: ключ. Ошибок нет (ключ не выполнялся — ничего).
    pub fn done(&mut self, k: K) {
        if !self.running.remove(&k) {
            return;
        }
        if self.waiting.get(&k).is_some_and(|n| *n > 0) {
            self.ready.push_back(k);
        }
    }

    /// Что делает: сколько полос выполняется сейчас. Ошибок нет.
    pub fn running(&self) -> usize {
        self.running.len()
    }

    /// Что делает: выполняется ли сейчас работа хоть одного ключа, для которого `hit` — да. Ошибок нет.
    pub fn running_any(&self, hit: impl Fn(K) -> bool) -> bool {
        self.running.iter().any(|k| hit(*k))
    }

    /// Что делает: сколько работ ждёт запуска у ключа `k` (без выполняющейся). Ошибок нет.
    pub fn waiting(&self, k: K) -> usize {
        self.waiting.get(&k).copied().unwrap_or(0)
    }

    /// Что делает: ничего не выполняется и ничего не ждёт. Ошибок нет.
    pub fn idle(&self) -> bool {
        self.running.is_empty() && self.waiting.is_empty()
    }
}
