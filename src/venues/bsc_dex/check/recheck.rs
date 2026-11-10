//! Файл: venues/bsc_dex/check/recheck.rs
//! Что делает: очереди внеочередных проверок для трёх циклов (налоги, крючки, сверка с квотером). Кнопка
//! «Перепроверить» (recheck_coins) кладёт в каждую очередь ТОЛЬКО переданные монеты — цикл проверяет их, а не все
//! монеты подписки (иначе одна кнопка запускала проверку сотен монет залпом). Загрузка истории V4 сообщает «новые
//! пулы» — циклы крючков и квотера проверяют пулы без итога. Полный проход — только по расписанию
//! (tax.recheck_interval_s, hooks.recheck_interval_s, dex.quoter_check_s). Все запросы к узлу идут в общем темпе
//! node/pace.rs (bsc.node_http_rps).
//! Сигнал хранится (Notify::notify_one): если цикл занят, он увидит очередь, когда освободится.
//!
//! Кто вызывает: mod.rs (recheck_coins), pools/history_v4.rs (новые пулы), exec/executor/remeasure.rs и
//! exec/ur_remeasure.rs (coins_by — откат продажи выхода), check/tax_cycle.rs, check/cycle.rs, check/quoter_cycle.rs
//! (забирают).
//! От чего зависит: alloy-primitives (Address).
//! Решение 211: монеты в очереди — адреса контрактов (раньше тикеры: показное имя монеты может отличаться от тикера
//! пулов и меняться).
//! Решения 247/252, дополнение (Тигран 10.11: согласовано; задача t77): перепроверка налогов после отката продажи
//! выхода «мало на выходе» (exec/executor/remeasure.rs, exec/ur_remeasure.rs) — кошельком сделки: очередь налогов
//! помнит счёт DEX сделки у такой монеты (coins_by), цикл налогов пробует ею (check/tax_cycle.rs). Кошелёк строки
//! «Пробы» и кошелёк сделки — разные адреса (факт t71), налог reflection-монет может зависеть от отправителя и его
//! баланса. Кнопка «Перепроверить», «пул готов» и полный проход — как раньше, кошельком строки «Пробы».
//! t78 (дополнение 10.11, Тигран: согласовано): очередь помнит и id сделки (By) — итог кошелька сделки хранится только
//! для продажи этой сделки (check/deal_taxes.rs); общие налоги монеты та же очередь перепроверяет кошельком «Пробы».
//! Настроек нет.

use std::collections::{HashMap, HashSet};

use alloy_primitives::Address;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

/// Чьим кошельком перепроверить налоги монеты после отката продажи выхода: счёт DEX сделки (решение 132) и id сделки
/// (t78: итог — только для продажи этой сделки, check/deal_taxes.rs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct By {
    pub account: String,
    pub deal: String,
}

/// Очередь одного цикла.
#[derive(Clone, Default)]
pub struct Queue {
    /// Монеты, были ли новые пулы, кошелёк сделки у монеты (перепроверка кошельком сделки — только очередь налогов).
    #[allow(clippy::type_complexity)]
    inner: Arc<Mutex<(HashSet<Address>, bool, HashMap<Address, By>)>>,
    notify: Arc<Notify>,
}

impl Queue {
    /// Ждёт запроса. Ошибок нет.
    pub async fn notified(&self) {
        self.notify.notified().await;
    }

    /// Забирает запрос: адреса монет и были ли новые пулы. Ошибок нет.
    pub fn take(&self) -> (HashSet<Address>, bool) {
        let (coins, new_pools, _) = self.take_all();
        (coins, new_pools)
    }

    /// Забирает запрос целиком: адреса монет, были ли новые пулы и кошелёк сделки у монет, которые надо перепроверить
    /// и кошельком сделки (coins_by). Ошибок нет.
    pub fn take_all(&self) -> (HashSet<Address>, bool, HashMap<Address, By>) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        (
            std::mem::take(&mut g.0),
            std::mem::take(&mut g.1),
            std::mem::take(&mut g.2),
        )
    }

    /// Ставит монеты в очередь; `by` — кошелёк сделки (перепроверка и им, итог — для продажи сделки), None — только
    /// кошельком строки «Пробы». Ошибок нет.
    fn push(&self, coins: &[Address], new_pools: bool, by: Option<&By>) {
        {
            let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            g.0.extend(coins.iter().copied());
            g.1 |= new_pools;
            if let Some(b) = by {
                g.2.extend(coins.iter().map(|a| (*a, b.clone())));
            }
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
        self.coins_by(coins, None);
    }

    /// Как coins, налоги — ещё и кошельком сделки `by` (решения 247/252, дополнение 10.11: перепроверка после отката
    /// продажи выхода; t78 — итог кошелька сделки только для её продажи, общие — кошельком «Пробы»); None — как coins.
    /// Крючки и сверка с квотером от кошелька не зависят — как coins. Ошибок нет.
    pub fn coins_by(&self, coins: &[Address], by: Option<By>) {
        self.taxes.push(coins, false, by.as_ref());
        self.hooks.push(coins, false, None);
        self.quoter.push(coins, false, None);
    }

    /// Подключены новые пулы: проверить пулы без итога (крючки, сверка). Налоги — по сигналу «пул готов». Ошибок нет.
    pub fn new_pools(&self) {
        self.hooks.push(&[], true, None);
        self.quoter.push(&[], true, None);
    }
}
