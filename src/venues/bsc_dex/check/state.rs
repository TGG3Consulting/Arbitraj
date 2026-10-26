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
    /// Газ разрешений (ERC-20 approve на Permit2 + Permit2.approve) для покупки (USDT) и продажи (монета) —
    /// тратится, только если разрешения ещё нет (решение 35: «газ approve, первый раз на монету»).
    pub gas_approve_buy: Option<u64>,
    pub gas_approve_sell: Option<u64>,
    /// Влияние крючка (только V4 группы B/C).
    pub hook: Option<PoolCheck>,
    /// Проверка крючка не прошла из-за незаданной настройки — её имя (из ошибки хранилища): проверка повторяется при
    /// задании именно её (решение 75).
    pub hook_wait: Option<String>,
    /// Сверка с квотером по сторонам (решение 73, quoter_sides.rs): у каждой — None (ещё не было или сторона мелкая)
    /// или Some(Err) — расхождение больше допуска (причина).
    pub quoter: super::quoter_sides::QuoterSides,
    /// Нативная монета (решение 62, check/native_probe.rs): газ обёртки сверх обмена токеном — (покупка с
    /// UNWRAP_WETH, продажа с WRAP_ETH); None — не мерился; Some(Err) — проба не прошла (причина).
    pub wrap_gas: Option<Result<(u64, u64), String>>,
}

/// Хранилище итогов проверок. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Checks {
    coins: Arc<RwLock<HashMap<String, CoinCheck>>>,
    pools: Arc<RwLock<HashMap<PoolRef, PoolFacts>>>,
    /// Газ перевода монеты из кошелька (gasUsed пробного перевода) — для стоимости вывода.
    transfer_gas: Arc<RwLock<HashMap<String, u64>>>,
    /// Газ нативного перевода BNB из кошелька (решение 62) — пробный перевод с value, check/native_probe.rs.
    native_transfer_gas: Arc<RwLock<HashMap<String, u64>>>,
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

    /// Записывает газ перевода монеты. Ошибок нет.
    pub fn set_transfer_gas(&self, ticker: &str, gas: u64) {
        self.transfer_gas
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(ticker.to_string(), gas);
    }

    /// Газ перевода монеты. None — не измерен.
    pub fn transfer_gas(&self, ticker: &str) -> Option<u64> {
        self.transfer_gas
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(ticker)
            .copied()
    }

    /// Записывает газ нативного перевода монеты (решение 62). Ошибок нет.
    pub fn set_native_transfer_gas(&self, ticker: &str, gas: u64) {
        self.native_transfer_gas
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(ticker.to_string(), gas);
    }

    /// Газ нативного перевода монеты. None — не измерен.
    pub fn native_transfer_gas(&self, ticker: &str) -> Option<u64> {
        self.native_transfer_gas
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(ticker)
            .copied()
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
