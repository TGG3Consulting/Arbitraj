//! Файл: venues/bsc_dex/pools/load_balancer_more.rs
//! Что делает: решение 473 (T450-03…07) — пачка чтения пула Balancer V2 вида dex.balancer_pool и её разбор: к пяти
//! общим вызовам (load_balancer::calls — getPoolTokens, комиссия, пауза пула, параметр кривой, пауза хранилища)
//! добавляются вызовы вида — composable stable: getScalingFactors, getTokenRateCache и getRate поставщика каждого токена
//! с курсом, время блока (Multicall3); meta-stable: getPriceRateCache и getRate, время блока; stable phantom:
//! getScalingFactor каждого токена; LBP: getSwapEnabled; investment: getSwapEnabled, getCollectedManagementFees;
//! managed: getSwapEnabled, getActualSupply, getCircuitBreakerState каждого токена (кроме BPT); linear:
//! getWrappedTokenRate (и totalSupply у выпуска linear). Разбор собирает кривую (load_balancer_more_curve.rs) и
//! состояние load_balancer::state.
//! Кто вызывает: pools/load_balancer.rs (calls / decode — ветка Fixed.more).
//! От чего зависит: abi_balancer.rs, abi_balancer_more.rs, load_balancer_more_fixed.rs (More),
//! load_balancer_more_curve.rs, load_lb.rs (time_call — Multicall3.getCurrentBlockTimestamp).
//! Настройки: нет (dex.multicall3 — в More.time).
//!
//! Источник — шапки abi_balancer_more.rs и load_balancer_more_curve.rs.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use super::abi_balancer::{IBalancerPool as P, IBalancerVault as V};
use super::abi_balancer_more::{
    IComposable as C, ILinear as L, IMetaStable as M, IPhantom as Ph, IRateProvider as R,
    ISmartWeighted as W,
};
use super::balancer_pool_type::PoolType;
use super::load_balancer_more_fixed::More;

/// Вызов без аргументов к контракту. Ошибок нет.
fn at<T: SolCall>(to: Address, c: T) -> (Address, Bytes) {
    (to, c.abi_encode().into())
}

/// Пачка чтения пула вида (порядок — как разбирает load_balancer_more_curve.rs). Принимает: хранилище, пул, id пула,
/// неизменяемое вида. Возвращает: вызовы. Ошибок нет.
pub fn calls(
    vault: Address,
    pool: Address,
    id: alloy_primitives::B256,
    m: &More,
) -> Vec<(Address, Bytes)> {
    let curve = if m.ty.weighted() {
        at(pool, P::getNormalizedWeightsCall {})
    } else if m.ty.linear() {
        at(pool, L::getTargetsCall {})
    } else {
        at(pool, P::getAmplificationParameterCall {})
    };
    let mut l = vec![
        at(vault, V::getPoolTokensCall { poolId: id }),
        at(pool, P::getSwapFeePercentageCall {}),
        at(pool, P::getPausedStateCall {}),
        curve,
        at(vault, P::getPausedStateCall {}),
    ];
    match m.ty {
        PoolType::Composable | PoolType::MetaStable => {
            if m.ty == PoolType::Composable {
                l.push(at(pool, C::getScalingFactorsCall {}));
            }
            for (t, p) in rated(m) {
                l.push(if m.ty == PoolType::Composable {
                    at(pool, C::getTokenRateCacheCall { token: t })
                } else {
                    at(pool, M::getPriceRateCacheCall { token: t })
                });
                l.push(at(p, R::getRateCall {}));
            }
            if let Some(mc) = m.time.filter(|_| !rated(m).is_empty()) {
                l.push(super::load_lb::time_call(mc));
            }
        }
        PoolType::Phantom => {
            for t in &m.tokens {
                l.push(at(pool, Ph::getScalingFactorCall { token: *t }));
            }
        }
        PoolType::Lbp => l.push(at(pool, W::getSwapEnabledCall {})),
        PoolType::Investment => {
            l.push(at(pool, W::getSwapEnabledCall {}));
            l.push(at(pool, W::getCollectedManagementFeesCall {}));
        }
        PoolType::Managed => {
            l.push(at(pool, W::getSwapEnabledCall {}));
            l.push(at(pool, W::getActualSupplyCall {}));
            for (k, t) in m.tokens.iter().enumerate() {
                if Some(k) != m.bpt {
                    l.push(at(pool, W::getCircuitBreakerStateCall { token: *t }));
                }
            }
        }
        PoolType::LinearV1 | PoolType::Linear => {
            l.push(at(pool, L::getWrappedTokenRateCall {}));
            if m.ty == PoolType::Linear {
                l.push(at(pool, L::totalSupplyCall {}));
            }
        }
    }
    l
}

/// Токены с поставщиком курса и их поставщики (по порядку токенов). Ошибок нет.
pub fn rated(m: &More) -> Vec<(Address, Address)> {
    m.tokens
        .iter()
        .zip(&m.providers)
        .filter(|(_, p)| !p.is_zero())
        .map(|(t, p)| (*t, *p))
        .collect()
}
