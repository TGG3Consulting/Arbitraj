//! Файл: venues/bsc_dex/pools/load_balancer_more_curve.rs
//! Что делает: решение 473 (T450-03…07) — разбор пачки чтения пула Balancer V2 вида dex.balancer_pool
//! (load_balancer_more::calls) в состояние VaultPool: кривая вида (VaultCurve::V2 или весовая у LBP / investment),
//! множители с курсом на блоке чтения, пауза (и swapEnabled = нет — как пауза), балансы investment без собранных
//! комиссий управляющего. Пара с BPT пула (composable stable, stable phantom, managed) не берётся.
//! Кто вызывает: pools/load_balancer.rs (decode — ветка Fixed.more), тесты на записанных ответах; части кривой —
//! load_balancer_more_parts.rs.
//! От чего зависит: core/types (VaultCurve, V2Curve), abi_balancer*.rs, load_balancer.rs (state), load_lb.rs
//! (decode_time), math/balancer_math.rs (mulDown).
//! Настройки: нет.
//!
//! Источник: курс в обмене — composable stable: ComposableStablePoolRates.sol (20240223-v6) L241-L257
//! (_cacheTokenRateIfNecessary: block.timestamp > expires — курс поставщика getRate, иначе кеш; до расчёта, из
//! _beforeSwapJoinExit — ComposableStablePool.sol L159-L167), L296-L306 (множитель = decimals·mulDown(курс)),
//! PriceRateCache.sol L89, L103 (курс ≥ 2^96 — откат PRICE_RATE_OVERFLOW); meta-stable: MetaStablePool.sol (20210727)
//! L125-L133 (кеш до расчёта), L424-L446, L519-L538, L606-L618 (курс ≥ 2^128 — откат); stable phantom:
//! StablePhantomPool.sol (20211208) L207-L215 (кеш обновляется ПОСЛЕ расчёта множителей в BaseGeneralPool.onSwap —
//! обмен идёт по множителю getScalingFactor на блоке), L615-L650. LBP: LiquidityBootstrappingPool.sol (20210721)
//! L315-L333 (SWAPS_DISABLED), веса getNormalizedWeights — на времени блока. Investment: InvestmentPool.sol (20210907)
//! L342-L360, L518-L544 (баланс − собранная комиссия управляющего, downscaleDown). Managed: ManagedPool.sol
//! (20221021) L123-L137 (SWAPS_DISABLED), ManagedPoolSettings.sol L743-L774 (границы цены BPT × множитель токена —
//! здесь делятся обратно: множитель = 10^(18 − decimals) точно). Linear: LinearPool.sol (20230410-v5) L526-L554
//! (множитель обёрнутого = decimals·mulDown(getWrappedTokenRate), у BPT — 1e18), L733-L749 (виртуальный выпуск =
//! totalSupply − баланс BPT); 20211208 / 20220404 / 20220425 — LinearPool.sol L665-L668 (2^112 − 1 − баланс BPT).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::pool_vault::V2Curve;
use crate::core::types::{PoolState, VaultCurve};

use super::abi_balancer::{IBalancerPool as P, IBalancerVault as V};
use super::abi_balancer_more::{ILinear as L, ISmartWeighted as W};
use super::balancer_pool_type::PoolType;
use super::load_balancer_fixed::Fixed;
use super::load_balancer_more_fixed::More;
use super::load_balancer_more_parts as parts;

/// Ответы пачки по порядку.
pub struct Cur<'a> {
    pub got: &'a [Result<Bytes, String>],
    pub at: usize,
}

impl Cur<'_> {
    /// Следующий ответ, разобранный по ABI вызова. Ошибки (текст): откат, нет ответа, не по ABI.
    pub fn take<T: SolCall>(&mut self) -> Result<T::Return, String> {
        let i = self.at;
        self.at += 1;
        let b = match self.got.get(i) {
            Some(Ok(b)) => b,
            Some(Err(r)) => return Err(format!("вызов {i} ({}) откатился: {r}", T::SIGNATURE)),
            None => return Err(format!("нет ответа на вызов {i} ({})", T::SIGNATURE)),
        };
        T::abi_decode_returns(b).map_err(|e| format!("{} не по ABI: {e}", T::SIGNATURE))
    }
}

/// Разбор пачки (без сети; её же зовут тесты). Принимает: неизменяемое пула, его вид, ответы по порядку
/// load_balancer_more::calls, пару. Возвращает: VaultPool. Ошибки (текст): откат / не по ABI, токены пула изменились,
/// точность A ≠ 1000, нет времени блока (dex.multicall3 не задана) у пула с курсами, курс больше предела кеша, пара с
/// BPT пула, несогласованное состояние (load_balancer::state).
pub fn decode(
    fx: &Fixed,
    m: &More,
    got: &[Result<Bytes, String>],
    pair: (Address, Address),
) -> Result<PoolState, String> {
    let mut c = Cur { got, at: 0 };
    let pt = c.take::<V::getPoolTokensCall>()?;
    if pt.tokens != m.tokens {
        // Токены изменились (managed: TokenAdded / TokenRemoved) — неизменяемое сбросить, перечитать с узла.
        super::load_balancer_fixed::forget(super::load_balancer_fixed::pool_of(fx.id));
        return Err(super::load_balancer_fixed::TOKENS_CHANGED.into());
    }
    let fee = c.take::<P::getSwapFeePercentageCall>()?;
    let mut paused = c.take::<P::getPausedStateCall>()?.paused;
    let (mut amp, mut weights, mut targets) = (U256::ZERO, Vec::new(), (U256::ZERO, U256::ZERO));
    if m.ty.weighted() {
        weights = c.take::<P::getNormalizedWeightsCall>()?;
    } else if m.ty.linear() {
        let t = c.take::<L::getTargetsCall>()?;
        targets = (t.lowerTarget, t.upperTarget);
    } else {
        let a = c.take::<P::getAmplificationParameterCall>()?;
        if a.precision != U256::from(1_000u32) {
            return Err(format!(
                "точность A {} ≠ 1000 (StableMath _AMP_PRECISION)",
                a.precision
            ));
        }
        amp = a.value;
    }
    paused |= c.take::<P::getPausedStateCall>()?.paused;
    if m.ty.bpt_excluded()
        && m.bpt
            .is_some_and(|b| [pair.0, pair.1].contains(&m.tokens[b]))
    {
        return Err("пара с BPT пула — вход / выход ликвидности, не обмен (не считается)".into());
    }
    let scaling: Option<Vec<U256>> = m
        .tokens
        .iter()
        .map(|t| fx.scaling.get(t).copied())
        .collect();
    let scaling = scaling.ok_or("нет множителя токена пула")?;
    let mut balances = pt.balances;
    let bpt = m.bpt.map(|b| b as u8);
    let curve = match m.ty {
        PoolType::Composable | PoolType::MetaStable | PoolType::Phantom => {
            let (factors, alt) = parts::factors(m, &mut c)?;
            let v2 = m.ty == PoolType::Composable;
            let bpt = if m.ty == PoolType::MetaStable {
                None
            } else {
                bpt
            };
            let cv = V2Curve::StableRated {
                amp,
                factors,
                bpt,
                v2,
                alt,
            };
            VaultCurve::V2(Box::new(cv))
        }
        PoolType::Lbp => {
            paused |= !c.take::<W::getSwapEnabledCall>()?;
            VaultCurve::Weighted { weights }
        }
        PoolType::Investment => {
            paused |= !c.take::<W::getSwapEnabledCall>()?;
            let f = c.take::<W::getCollectedManagementFeesCall>()?;
            if f.tokens != m.tokens || f.collectedFees.len() != balances.len() {
                return Err("getCollectedManagementFees: токены не те, что у пула".into());
            }
            for (b, x) in balances.iter_mut().zip(&f.collectedFees) {
                let w = "собранная комиссия больше баланса — обмен откатится";
                *b = b.checked_sub(*x).ok_or(w)?;
            }
            VaultCurve::Weighted { weights }
        }
        PoolType::Managed => {
            paused |= !c.take::<W::getSwapEnabledCall>()?;
            parts::managed(m, &mut c, weights, &scaling)?
        }
        PoolType::LinearV1 | PoolType::Linear => parts::linear(m, &mut c, targets, &balances)?,
    };
    super::load_balancer::state(
        fx.id, &m.tokens, balances, scaling, fee, paused, curve, pair,
    )
}
