//! Файл: venues/bsc_dex/pools/load_balancer_v3_extra.rs
//! Что делает: решение 452 — неизменяемое пула Balancer V3, своё у каждого выпуска (второй проход чтения
//! load_balancer_v3_fixed.rs): веса и нижние балансы весового пула (решение 441), √α / √β Gyro 2-CLP, параметры Gyro
//! E-CLP, токен проекта / запрет / виртуальный резерв LBP, цена LBP с фиксированной ценой, предел доли сделки QuantAMM.
//! Вызовы и разбор — без сети (их же зовут тесты на записанных ответах).
//! Кто вызывает: pools/load_balancer_v3_fixed.rs, тесты.
//! От чего зависит: abi_balancer_v3.rs (IBalancerV3Pool), abi_balancer_v3_kinds.rs, balancer_v3_factories.rs, core/types.
//! Настройки: нет.
//!
//! Источник: шапки abi_balancer_v3.rs, abi_balancer_v3_kinds.rs; неизменяемость — Gyro2CLPPool.sol L36-L37 (_sqrtAlpha,
//! _sqrtBeta immutable), GyroECLPPool.sol L42-L60 (immutable), LBPool.sol (индексы токенов, запрет, виртуальный резерв —
//! immutable), FixedPriceLBPool.sol L53 (_projectTokenRate immutable), QuantAMMWeightedPool.sol L699-L763
//! (maxTradeSizeRatio задаётся один раз в initialize).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::pool_vault::EclpParams;

use super::abi_balancer_v3::IBalancerV3Pool as P;
use super::abi_balancer_v3_kinds::{IGyro2Clp, IGyroEclp, ILbp, ILbpV1, ILbpV2, IQuantAmm};
use super::balancer_v3_factories::Release;

/// Неизменяемое сверх общего (по выпуску).
#[derive(Debug, Clone, PartialEq)]
pub enum Extra {
    /// Нет (стейблсвап, ReClamm — всё читается каждым блоком).
    None,
    /// Весовой пул: веса и (20260115) нижние балансы.
    Weighted { weights: Vec<U256>, min: Vec<U256> },
    /// Gyro 2-CLP: √α, √β.
    Gyro2 { sqrt_alpha: U256, sqrt_beta: U256 },
    /// Gyro E-CLP.
    Eclp(Box<EclpParams>),
    /// LBP: индекс токена проекта, запрет продажи токена проекта, виртуальный резерв (18 знаков; v1 / v2 — 0), нижние
    /// балансы (v4).
    Lbp {
        project: u8,
        block_in: bool,
        virt: U256,
        min: Vec<U256>,
    },
    /// LBP с фиксированной ценой: индекс токена проекта, цена (18 знаков).
    FixedLbp { project: u8, rate: U256 },
    /// QuantAMM: maxTradeSizeRatio.
    QuantAmm { max_trade: U256 },
}

/// Вызовы второго прохода по выпуску (порядок — как разбирает decode). Ошибок нет.
pub fn calls(pool: Address, r: Release) -> Vec<(Address, Bytes)> {
    let c = |d: Vec<u8>| (pool, Bytes::from(d));
    match r {
        Release::Weighted => vec![c(P::getNormalizedWeightsCall {}.abi_encode())],
        Release::WeightedMin => vec![
            c(P::getNormalizedWeightsCall {}.abi_encode()),
            c(P::getMinTokenBalancesCall {}.abi_encode()),
        ],
        Release::Gyro2Clp => vec![c(
            IGyro2Clp::getGyro2CLPPoolImmutableDataCall {}.abi_encode()
        )],
        Release::GyroEclp => vec![c(IGyroEclp::getECLPParamsCall {}.abi_encode())],
        Release::LbpV1 => vec![c(ILbpV1::getLBPoolImmutableDataCall {}.abi_encode())],
        Release::LbpV2 => vec![c(ILbpV2::getLBPoolImmutableDataCall {}.abi_encode())],
        Release::LbpV3 | Release::LbpV4 => {
            let mut l = vec![
                c(ILbp::getTokenIndicesCall {}.abi_encode()),
                c(ILbp::isProjectTokenSwapInBlockedCall {}.abi_encode()),
                c(ILbp::getReserveTokenVirtualBalanceCall {}.abi_encode()),
            ];
            if r == Release::LbpV4 {
                l.push(c(P::getMinTokenBalancesCall {}.abi_encode()));
            }
            l
        }
        Release::FixedLbp => vec![
            c(ILbp::getTokenIndicesCall {}.abi_encode()),
            c(ILbp::getProjectTokenRateCall {}.abi_encode()),
        ],
        Release::QuantAmm => {
            vec![c(
                IQuantAmm::getQuantAMMWeightedPoolImmutableDataCall {}.abi_encode()
            )]
        }
        Release::Stable | Release::StableOpen | Release::StableGuard => Vec::new(),
        Release::ReClamm | Release::ReClammLive => Vec::new(),
    }
}

/// Разбор ответов второго прохода. Принимает: выпуск, ответы (по порядку calls). Возвращает: неизменяемое. Ошибки
/// (текст): откат, ответ не по ABI, индекс токена проекта не 0 / 1.
pub fn decode(r: Release, got: &[Result<Bytes, String>]) -> Result<Extra, String> {
    let ok = |i: usize| match got.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(w)) => Err(format!("неизменяемое пула: вызов {i} откатился: {w}")),
        None => Err(format!("неизменяемое пула: нет ответа на вызов {i}")),
    };
    let dec = |e: alloy::sol_types::Error| format!("неизменяемое пула не по ABI: {e}");
    let idx = |v: U256| -> Result<u8, String> {
        if v < U256::from(2u8) {
            Ok(v.to::<u8>())
        } else {
            Err(format!("индекс токена проекта {v} не 0 / 1"))
        }
    };
    Ok(match r {
        Release::Weighted | Release::WeightedMin => {
            let weights = P::getNormalizedWeightsCall::abi_decode_returns(ok(0)?).map_err(dec)?;
            let min = if r == Release::WeightedMin {
                P::getMinTokenBalancesCall::abi_decode_returns(ok(1)?).map_err(dec)?
            } else {
                Vec::new()
            };
            Extra::Weighted { weights, min }
        }
        Release::Gyro2Clp => {
            let d = IGyro2Clp::getGyro2CLPPoolImmutableDataCall::abi_decode_returns(ok(0)?)
                .map_err(dec)?;
            Extra::Gyro2 {
                sqrt_alpha: d.sqrtAlpha,
                sqrt_beta: d.sqrtBeta,
            }
        }
        Release::GyroEclp => {
            let e = IGyroEclp::getECLPParamsCall::abi_decode_returns(ok(0)?).map_err(dec)?;
            let (p, d) = (e.params, e.d);
            Extra::Eclp(Box::new(EclpParams {
                alpha: p.alpha,
                beta: p.beta,
                c: p.c,
                s: p.s,
                lambda: p.lambda,
                tau_alpha_x: d.tauAlpha.x,
                tau_alpha_y: d.tauAlpha.y,
                tau_beta_x: d.tauBeta.x,
                tau_beta_y: d.tauBeta.y,
                u: d.u,
                v: d.v,
                w: d.w,
                z: d.z,
                d_sq: d.dSq,
            }))
        }
        Release::LbpV1 | Release::LbpV2 => {
            let (project, block_in) = if r == Release::LbpV1 {
                let call = ILbpV1::getLBPoolImmutableDataCall::abi_decode_returns;
                let d = call(ok(0)?).map_err(dec)?;
                (d.projectTokenIndex, d.isProjectTokenSwapInBlocked)
            } else {
                let call = ILbpV2::getLBPoolImmutableDataCall::abi_decode_returns;
                let d = call(ok(0)?).map_err(dec)?;
                (d.projectTokenIndex, d.isProjectTokenSwapInBlocked)
            };
            Extra::Lbp {
                project: idx(project)?,
                block_in,
                virt: U256::ZERO,
                min: Vec::new(),
            }
        }
        Release::LbpV3 | Release::LbpV4 => {
            let t = ILbp::getTokenIndicesCall::abi_decode_returns(ok(0)?).map_err(dec)?;
            let blocked = ILbp::isProjectTokenSwapInBlockedCall::abi_decode_returns;
            let block_in = blocked(ok(1)?).map_err(dec)?;
            let virt = ILbp::getReserveTokenVirtualBalanceCall::abi_decode_returns;
            let v = virt(ok(2)?).map_err(dec)?;
            let min = if r == Release::LbpV4 {
                P::getMinTokenBalancesCall::abi_decode_returns(ok(3)?).map_err(dec)?
            } else {
                Vec::new()
            };
            Extra::Lbp {
                project: idx(t.projectTokenIndex)?,
                block_in,
                virt: v.scaled18,
                min,
            }
        }
        Release::FixedLbp => {
            let t = ILbp::getTokenIndicesCall::abi_decode_returns(ok(0)?).map_err(dec)?;
            let rate = ILbp::getProjectTokenRateCall::abi_decode_returns(ok(1)?).map_err(dec)?;
            Extra::FixedLbp {
                project: idx(t.projectTokenIndex)?,
                rate,
            }
        }
        Release::QuantAmm => {
            let call = IQuantAmm::getQuantAMMWeightedPoolImmutableDataCall::abi_decode_returns;
            let d = call(ok(0)?).map_err(dec)?;
            Extra::QuantAmm {
                max_trade: d.maxTradeSizeRatio,
            }
        }
        Release::Stable | Release::StableOpen | Release::StableGuard => Extra::None,
        Release::ReClamm | Release::ReClammLive => Extra::None,
    })
}
