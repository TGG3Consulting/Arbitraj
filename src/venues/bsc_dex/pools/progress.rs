//! Файл: venues/bsc_dex/pools/progress.rs
//! Что делает: сигналы слушателей пулов для проверок и журнала одной подписки на рынок:
//! - стартовая загрузка: сколько стартовых пачек V2/V3 ещё не закончили первую загрузку состояния — первый проход
//!   налогов ждёт её (пулы V4 подгружаются в фоне, их монеты проверяются по мере появления пулов);
//! - «пул готов»: пул стал достоверным (прочитан, перечитан) — монеты, ждущие пула, проверяются сразу, а не через
//!   tax.recheck_interval_s;
//! - «пул снова не пуст»: пустой пул (quote/emptiness.rs) получил ликвидность — сверка с квотером только его, сразу;
//! - журнал без спама: одна запись на вид причины (числа в тексте — блоки, адреса — не делают причину новой);
//!   reason_key — та же «причина без цифр» для записи только при смене состояния (check/quoter_cycle.rs и др.).
//!
//! Кто вызывает: pools/market.rs (создаёт счётчик), pools/listen.rs, pools/listen_log.rs (сигналят), check/cycle.rs (ждёт).
//! От чего зависит: ctx.rs.
//! Настройки: нет.

use std::collections::HashSet;

use crate::core::types::PoolRef;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::storage::journal::Level;

use super::super::ctx::Ctx;

/// Сигналы одной подписки. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Signals {
    /// Стартовых пачек V2/V3 без первой загрузки.
    left: Arc<AtomicUsize>,
    started: Arc<Notify>,
    /// Пул стал достоверным.
    pub ready: Arc<Notify>,
    seen: Arc<Mutex<HashSet<String>>>,
    /// Пустые пулы, снова получившие ликвидность, и сигнал о них.
    refilled: Arc<Mutex<HashSet<PoolRef>>>,
    pub refill: Arc<Notify>,
    /// Связь слушателей с узлом WS (link.rs).
    pub link: Arc<Mutex<super::link::State>>,
}

impl Signals {
    /// Добавляет стартовые пачки (до запуска их слушателей). Ошибок нет.
    pub fn expect(&self, n: usize) {
        self.left.fetch_add(n, Ordering::SeqCst);
    }

    /// Стартовая пачка закончила первую загрузку (успешно или нет). Ошибок нет.
    pub fn loaded(&self) {
        // Уменьшение без ухода ниже нуля через compare_exchange: fetch_update в новом stable Rust устарел
        // (переименован в try_update), а try_update нет в старых версиях — так собирается на любой.
        let mut before = self.left.load(Ordering::SeqCst);
        while before > 0 {
            match self
                .left
                .compare_exchange(before, before - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(now) => before = now,
            }
        }
        if before <= 1 {
            self.started.notify_waiters();
        }
    }

    /// Ждёт конца стартовой загрузки. Ошибок нет.
    pub async fn wait_start(&self) {
        loop {
            let n = self.started.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            if self.left.load(Ordering::SeqCst) == 0 {
                return;
            }
            n.await;
        }
    }

    /// Пул стал достоверным. Ошибок нет.
    pub fn pool_ready(&self) {
        self.ready.notify_waiters();
    }

    /// Пустой пул снова не пуст: сверить его с квотером сразу. Ошибок нет.
    pub fn refilled(&self, r: PoolRef) {
        self.refilled
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(r);
        self.refill.notify_waiters();
    }

    /// Забирает пулы, снова получившие ликвидность. Ошибок нет.
    pub fn take_refilled(&self) -> Vec<PoolRef> {
        let mut set = self.refilled.lock().unwrap_or_else(|p| p.into_inner());
        set.drain().collect()
    }

    /// Запись в журнал, если такой вид причины ещё не записан (цифры в тексте не учитываются). Ошибок нет.
    pub async fn once(&self, ctx: &Ctx, level: Level, kind: &str, msg: String) {
        let key = reason_key(&format!("{kind}:{msg}"));
        let fresh = self
            .seen
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key);
        if fresh {
            ctx.log(level, kind, msg, None).await;
        }
    }
}

/// Вид причины: текст без цифр (блоки, суммы, проценты не делают причину новой). Ошибок нет.
pub fn reason_key(s: &str) -> String {
    s.chars().filter(|c| !c.is_ascii_digit()).collect()
}
