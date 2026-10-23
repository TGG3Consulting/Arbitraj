//! Файл: venues/bsc_dex/check/state.rs
//! Что делает: итоги проверок в памяти — налоги монет (CoinCheck), влияние крючков пулов (PoolCheck), газ обмена
//! по пулу из пробного обмена (покупка, продажа), итог сверки пула с квотером. По ним котировка решает, какие
//! пулы можно использовать: пока проверка не закончена — торговли нет (решения 30, 32).
//! Кто вызывает: check/cycle.rs, check/taxes.rs, check/hooks_probe.rs, quote/quoter.rs (пишут), quote/direct.rs (читает).
//! От чего зависит: core/types (CoinCheck, PoolCheck, PoolRef).
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::core::types::{CoinCheck, PoolCheck, PoolRef};

/// Факты о пуле из проверок.
#[derive(Debug, Clone, Default)]
pub struct PoolFacts {
    /// Газ обмена USDT → монета (gasUsed пробного обмена).
    pub gas_buy: Option<u64>,
    /// Газ обмена монета → USDT.
    pub gas_sell: Option<u64>,
    /// Влияние крючка (только V4 группы B/C).
    pub hook: Option<PoolCheck>,
    /// Сверка с квотером: None — ещё не было; Some(Err) — расхождение больше допуска (причина).
    pub quoter: Option<Result<(), String>>,
}

/// Хранилище итогов проверок. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Checks {
    coins: Arc<RwLock<HashMap<String, CoinCheck>>>,
    pools: Arc<RwLock<HashMap<PoolRef, PoolFacts>>>,
}

impl Checks {
    /// Записывает проверку монеты (по тикеру). Ошибок нет.
    pub fn set_coin(&self, c: CoinCheck) {
        self.coins
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(c.asset.ticker.clone(), c);
    }

    /// Проверка монеты. None — не было.
    pub fn coin(&self, ticker: &str) -> Option<CoinCheck> {
        self.coins
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(ticker)
            .cloned()
    }

    /// Факты о пуле (пустые, если проверок не было). Ошибок нет.
    pub fn pool(&self, r: &PoolRef) -> PoolFacts {
        self.pools
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(r)
            .cloned()
            .unwrap_or_default()
    }

    /// Изменяет факты о пуле. Принимает: ссылку и функцию. Ошибок нет.
    pub fn update_pool(&self, r: &PoolRef, f: impl FnOnce(&mut PoolFacts)) {
        let mut m = self.pools.write().unwrap_or_else(|p| p.into_inner());
        f(m.entry(r.clone()).or_default());
    }
}
