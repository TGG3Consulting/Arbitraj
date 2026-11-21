//! Файл: venues/bsc_dex/pools/balancer_pool_type.rs
//! Что делает: решение 473 (T450-03…07) — вид пулов фабрики ключа DEX Balancer V2 (настройка dex.balancer_pool):
//! composable stable v1–v6, stable phantom, meta-stable, LBP, investment, managed v1 / v2, linear (два выпуска по
//! виртуальному выпуску BPT). Не задана — как раньше: весовые пулы dex.factory и стейблсвап dex.balancer_stable_factory.
//! Кто вызывает: pools/load_balancer_fixed.rs, load_balancer.rs, load_balancer_more*.rs, per_block_balancer.rs,
//! verify_balancer.rs.
//! От чего зависит: ctx.rs, families.rs; варианты — settings/catalog/dex.rs BALANCER_POOL (тест сверяет оба списка).
//! Настройки: dex.balancer_pool [DEX].
//!
//! Источник — build-info balancer-deployments@b55dc2b8 (адреса фабрик Arbitrum — addresses/arbitrum.json):
//! composable_stable — 20220906-composable-stable-pool, 20221122-…-v2, 20230320-…-v4, 20230711-…-v5,
//! 20240223-…-v6 (обмен без BPT у всех один текст; 20230206-…-v3 в сети Arbitrum не развёрнут); stable_phantom —
//! 20211208-stable-phantom-pool; meta_stable — 20210727-meta-stable-pool; lbp — 20210721-liquidity-bootstrapping-pool,
//! 20211202-no-protocol-fee-lbp (LiquidityBootstrappingPool.sol один текст); investment — 20210907-investment-pool;
//! managed — 20221021-managed-pool, 20230411-managed-pool-v2; linear_v1 — 20211208-aave-linear-pool,
//! 20220404-erc4626-linear-pool-v2, 20220425-unbutton-aave-linear-pool (виртуальный выпуск = 2^112 − 1 − баланс BPT,
//! LinearPool.sol L67, _getApproximateVirtualSupply); linear — 20220817-aave-rebalanced-linear-pool,
//! 20230206-aave-rebalanced-linear-pool-v4, 20230213-yearn-linear-pool, 20230409-erc4626-linear-pool-v4,
//! 20230409-yearn-linear-pool-v2, 20230410-aave-linear-pool-v5 (виртуальный выпуск = totalSupply − баланс BPT).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::gateway::errors::ErrorCode;

use super::super::ctx::Ctx;
use super::families::Family;

/// Вид пулов фабрики ключа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolType {
    Composable,
    Phantom,
    MetaStable,
    Lbp,
    Investment,
    Managed,
    LinearV1,
    Linear,
}

/// Значения настройки по порядку вариантов (тот же список — settings/catalog/dex.rs BALANCER_POOL).
const NAMES: &[(&str, PoolType)] = &[
    ("composable_stable", PoolType::Composable),
    ("stable_phantom", PoolType::Phantom),
    ("meta_stable", PoolType::MetaStable),
    ("lbp", PoolType::Lbp),
    ("investment", PoolType::Investment),
    ("managed", PoolType::Managed),
    ("linear_v1", PoolType::LinearV1),
    ("linear", PoolType::Linear),
];

impl PoolType {
    /// Значение настройки. Ошибок нет.
    pub fn name(self) -> &'static str {
        NAMES
            .iter()
            .find(|(_, t)| *t == self)
            .map(|(n, _)| *n)
            .unwrap_or("")
    }

    /// Весовой пул (параметр кривой — getNormalizedWeights): LBP, investment, managed. Ошибок нет.
    pub fn weighted(self) -> bool {
        matches!(self, Self::Lbp | Self::Investment | Self::Managed)
    }

    /// Стейблсвап с курсами (параметр кривой — getAmplificationParameter). Ошибок нет.
    pub fn stable(self) -> bool {
        matches!(self, Self::Composable | Self::Phantom | Self::MetaStable)
    }

    /// Линейный пул (параметр кривой — getTargets). Ошибок нет.
    pub fn linear(self) -> bool {
        matches!(self, Self::LinearV1 | Self::Linear)
    }

    /// BPT пула среди его токенов, а обмен с ним — вход / выход ликвидности (_swapWithBpt, join / exit swap), который
    /// не считается: пары с BPT пула не берутся (composable stable, stable phantom, managed). У линейного пула обмен с
    /// BPT — LinearMath (считается). Ошибок нет.
    pub fn bpt_excluded(self) -> bool {
        matches!(self, Self::Composable | Self::Phantom | Self::Managed)
    }
}

/// Разбор значения настройки. Принимает: текст. Возвращает: вид или None — значение не из списка. Ошибок нет.
pub fn parse(s: &str) -> Option<PoolType> {
    NAMES.iter().find(|(n, _)| *n == s.trim()).map(|(_, t)| *t)
}

/// Вид пулов ключа DEX — dex.balancer_pool. Принимает: окружение, вид DEX. Возвращает: None — не задана (весовые /
/// стейблсвап, как раньше). Ошибки (текст): значение не из списка; настройка не читается (только «не задана» —
/// NotConfigured — значит None, как у dex.balancer_math).
pub fn of(ctx: &Ctx, f: &Family) -> Result<Option<PoolType>, String> {
    match ctx.dex_text("dex.balancer_pool", f.id) {
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(None),
        Err(e) => Err(format!("dex.balancer_pool [{}]: {}", f.id, e.message)),
        Ok(v) => parse(&v).map(Some).ok_or_else(|| {
            let known: Vec<&str> = NAMES.iter().map(|(n, _)| *n).collect();
            format!(
                "dex.balancer_pool [{}] = «{}»: известны {}",
                f.id,
                v.trim(),
                known.join(", ")
            )
        }),
    }
}

/// Вид пулов ключа DEX, прочитанный при последнем чтении неизменяемого пула (load_balancer_fixed.rs fixed) — по id
/// вида DEX (ключ области «DEX» с сетью). Нужен подписке на события (events_balancer.rs topics): у неё нет окружения
/// с настройками. Ключа без dex.balancer_pool здесь нет (BSC — прежние четыре события).
static SEEN: Mutex<Option<HashMap<&'static str, PoolType>>> = Mutex::new(None);

/// Запомнить вид ключа (None — снять). Принимает: вид DEX, вид пулов. Ошибок нет.
pub fn remember(f: &Family, ty: Option<PoolType>) {
    let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    match ty {
        Some(t) => m.insert(f.id, t),
        None => m.remove(f.id),
    };
}

/// Вид пулов ключа, если он уже читался с dex.balancer_pool. Принимает: вид DEX. Ошибок нет.
pub fn seen(f: &Family) -> Option<PoolType> {
    let g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().and_then(|m| m.get(f.id).copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::catalog::dex::BALANCER_POOL;

    #[test]
    fn names_match_catalog() {
        let ours: Vec<&str> = NAMES.iter().map(|(n, _)| *n).collect();
        assert_eq!(ours, BALANCER_POOL.to_vec());
        for (n, t) in NAMES {
            assert_eq!(parse(n), Some(*t));
            assert_eq!(t.name(), *n);
        }
    }
}
