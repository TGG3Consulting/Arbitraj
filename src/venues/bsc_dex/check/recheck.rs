//! Файл: venues/bsc_dex/check/recheck.rs
//! Что делает: очереди внеочередных проверок для трёх циклов (налоги, крючки, сверка с квотером). Кнопка
//! «Перепроверить» (recheck_coins) кладёт в каждую очередь ТОЛЬКО переданные монеты — цикл проверяет их, а не все
//! монеты подписки (иначе одна кнопка запускала проверку сотен монет залпом). Загрузка истории V4 сообщает «новые
//! пулы» — циклы крючков и квотера проверяют пулы без итога. Полный проход — только по расписанию
//! (tax.recheck_interval_s, hooks.recheck_interval_s, dex.quoter_check_s). Все запросы к узлу идут в общем темпе
//! node/pace.rs (bsc.node_http_rps).
//! Сигнал хранится (Notify::notify_one): если цикл занят, он увидит очередь, когда освободится.
//!
//! Кто вызывает: mod.rs (recheck_coins), pools/history_v4.rs (новые пулы), check/tax_cycle.rs, check/cycle.rs,
//! check/quoter_cycle.rs (забирают).
//! От чего зависит: alloy-primitives (Address).
//! Решение 211: монеты в очереди — адреса контрактов (раньше тикеры: показное имя монеты может отличаться от тикера
//! пулов и меняться).
//! Настроек нет.

use std::collections::HashSet;

use alloy_primitives::Address;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

/// Очередь одного цикла.
#[derive(Clone, Default)]
pub struct Queue {
    inner: Arc<Mutex<(HashSet<Address>, bool)>>,
    notify: Arc<Notify>,
}

impl Queue {
    /// Ждёт запроса. Ошибок нет.
    pub async fn notified(&self) {
        self.notify.notified().await;
    }

    /// Забирает запрос: адреса монет и были ли новые пулы. Ошибок нет.
    pub fn take(&self) -> (HashSet<Address>, bool) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        (std::mem::take(&mut g.0), std::mem::take(&mut g.1))
    }

    fn push(&self, coins: &[Address], new_pools: bool) {
        {
            let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            g.0.extend(coins.iter().copied());
            g.1 |= new_pools;
        }
        self.notify.notify_one();
    }
}

/// Очереди трёх циклов. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Recheck {
    pub taxes: Queue,
    pub hooks: Queue,
    pub quoter: Queue,
}

impl Recheck {
    /// Перепроверить эти монеты (налоги, крючки и сверку их пулов). Ошибок нет.
    pub fn coins(&self, coins: &[Address]) {
        self.taxes.push(coins, false);
        self.hooks.push(coins, false);
        self.quoter.push(coins, false);
    }

    /// Подключены новые пулы: проверить пулы без итога (крючки, сверка). Налоги — по сигналу «пул готов». Ошибок нет.
    pub fn new_pools(&self) {
        self.hooks.push(&[], true);
        self.quoter.push(&[], true);
    }
}
