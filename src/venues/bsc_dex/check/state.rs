//! Файл: venues/bsc_dex/check/state.rs
//! Что делает: итоги проверок в памяти — налоги монет (CoinCheck), влияние крючков пулов (PoolCheck), газ обмена
//! по пулу из пробного обмена (покупка, продажа), налоги монеты В КАЖДОМ пуле (решение 146), итог сверки пула с
//! квотером. По ним котировка решает, какие пулы можно использовать: пока проверка не закончена — торговли нет
//! (решения 30, 32).
//! Кто вызывает: check/cycle.rs, check/taxes.rs, check/hooks_probe.rs, quote/quoter.rs (пишут), quote/direct.rs,
//! quote/select.rs (читают).
//! От чего зависит: core/types (CoinCheck, PoolCheck, PoolRef, Side).
//! Настройки: нет.
//! Решение 211 (фаза 2b): монета — адрес контракта BSC. Налоги монеты и газ перевода хранятся по АДРЕСУ, а не по
//! тикеру: показное имя монеты ядра может меняться (тикер большинства площадок), а у одного адреса под разными
//! тикерами — одна проверка. Нативная монета (решение 62) — по адресу своей обёртки (dex.native_wrapped).
//! Решение 239: pinned — копия итогов проверок нужных токенов и пулов на момент снимка котировки
//! (venues/bsc_dex/frozen.rs): налоги монет и пулов, газ обменов и разрешений, крючки, сверка с квотером — котировка
//! по снимку читает их такими, какими они были в момент решения, а не после нового замера.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use alloy_primitives::Address;

use rust_decimal::Decimal;

use crate::core::types::{CoinCheck, PoolCheck, PoolRef, Side};

/// Налоги монеты пула (Entry.coin), измеренные пробным обменом ИМЕННО в этом пуле (решение 146, check/probe.rs): у
/// токенов с налогом «только при обмене через зарегистрированные пулы» (живой случай FLOKI 04.11: продажи на 0,30 %
/// хуже плана при налоге монеты 0) налог зависит от пула.
#[derive(Debug, Clone, PartialEq)]
pub struct PoolTaxes {
    /// Налог покупки, %: (ответ квотера − пришло монет) / ответ квотера.
    pub buy_pct: Decimal,
    /// Налог продажи, %: (ответ квотера на продажу − пришло встречного токена) / ответ квотера. None — НЕ ИЗМЕРЕН
    /// (продажа пробы откатилась или квотер не ответил) — это не 0: пул по продаже не используется.
    pub sell_pct: Option<Decimal>,
    /// Налог простого перевода на пустой адрес, % (от пула не зависит).
    pub transfer_pct: Decimal,
}

impl PoolTaxes {
    /// Налог нужной стороны (покупка / продажа монеты пула). None — не измерен. Ошибок нет.
    pub fn side(&self, side: Side) -> Option<Decimal> {
        match side {
            Side::Buy => Some(self.buy_pct),
            Side::Sell => self.sell_pct,
        }
    }
}

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
    /// Налоги монеты в этом пуле (решение 146, check/taxes.rs): пишутся по каждой удачной пробе пула; None — проба
    /// пула ещё не прошла (или пробный обмен в пуле откатился) — пул в котировку не идёт (quote/select.rs).
    pub taxes: Option<PoolTaxes>,
}

/// Хранилище итогов проверок. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Checks {
    coins: Arc<RwLock<HashMap<Address, CoinCheck>>>,
    pools: Arc<RwLock<HashMap<PoolRef, PoolFacts>>>,
    /// Газ перевода монеты из кошелька (gasUsed пробного перевода) — для стоимости вывода.
    transfer_gas: Arc<RwLock<HashMap<Address, u64>>>,
    /// Газ нативного перевода BNB из кошелька (решение 62) — пробный перевод с value, check/native_probe.rs.
    native_transfer_gas: Arc<RwLock<HashMap<Address, u64>>>,
}

impl Checks {
    /// Записывает проверку монеты — по адресу контракта (решение 211). Монета без адреса не записывается: налоги
    /// меряются пробным обменом в пуле, а пул всегда знает адрес токена. Ошибок нет.
    pub fn set_coin(&self, c: CoinCheck) {
        let Some(a) = c.asset.bsc_contract else {
            return;
        };
        self.coins
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(a, c);
    }

    /// Проверка монеты по адресу контракта. None — не было.
    pub fn coin(&self, token: &Address) -> Option<CoinCheck> {
        self.coins
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(token)
            .cloned()
    }

    /// Проверка монеты ядра (по её адресу; монета без адреса — None). Ошибок нет.
    pub fn coin_of(&self, asset: &crate::core::types::Asset) -> Option<CoinCheck> {
        asset.bsc_contract.and_then(|a| self.coin(&a))
    }

    /// Записывает газ перевода монеты (по адресу контракта). Ошибок нет.
    pub fn set_transfer_gas(&self, token: Address, gas: u64) {
        self.transfer_gas
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(token, gas);
    }

    /// Газ перевода монеты (по адресу контракта). None — не измерен.
    pub fn transfer_gas(&self, token: &Address) -> Option<u64> {
        self.transfer_gas
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(token)
            .copied()
    }

    /// Записывает газ нативного перевода монеты (решение 62; ключ — адрес обёртки). Ошибок нет.
    pub fn set_native_transfer_gas(&self, token: Address, gas: u64) {
        self.native_transfer_gas
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(token, gas);
    }

    /// Газ нативного перевода монеты (по адресу обёртки). None — не измерен.
    pub fn native_transfer_gas(&self, token: &Address) -> Option<u64> {
        self.native_transfer_gas
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(token)
            .copied()
    }

    /// Решение 239: копия итогов проверок на этот момент (снимок котировки, frozen.rs) — только переданные токены
    /// (проверка монеты, газ перевода) и пулы; своя память, новые замеры в неё не попадают. Чего не передали — в копии
    /// нет (котировка по снимку такой пул или монету не использует: «не проверено»). Принимает: адреса токенов, ссылки
    /// пулов. Возвращает: Checks. Ошибок нет.
    pub fn pinned<'a>(
        &self,
        tokens: &[Address],
        pools: impl IntoIterator<Item = &'a PoolRef>,
    ) -> Checks {
        let pick = |m: &RwLock<HashMap<Address, u64>>| -> HashMap<Address, u64> {
            let m = m.read().unwrap_or_else(|p| p.into_inner());
            tokens
                .iter()
                .filter_map(|t| m.get(t).map(|v| (*t, *v)))
                .collect()
        };
        let coins = {
            let m = self.coins.read().unwrap_or_else(|p| p.into_inner());
            tokens
                .iter()
                .filter_map(|t| m.get(t).map(|c| (*t, c.clone())))
                .collect()
        };
        let pools = {
            let m = self.pools.read().unwrap_or_else(|p| p.into_inner());
            pools
                .into_iter()
                .filter_map(|r| m.get(r).map(|f| (r.clone(), f.clone())))
                .collect()
        };
        Checks {
            coins: Arc::new(RwLock::new(coins)),
            pools: Arc::new(RwLock::new(pools)),
            transfer_gas: Arc::new(RwLock::new(pick(&self.transfer_gas))),
            native_transfer_gas: Arc::new(RwLock::new(pick(&self.native_transfer_gas))),
        }
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
