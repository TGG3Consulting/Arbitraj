//! Файл: venues/bsc_dex/pools/verify_calls.rs
//! Что делает: данные вызовов проверки пула V2/V3 (решение 58): поля пула по виду (factory, token0, token1; V3 — fee,
//! tickSpacing, lmPool; V2 со своей комиссией — метод dex.v2_fee_method) и реестр фабрики (getPair / getPool).
//! Кто вызывает: pools/verify.rs, pools/index_watch.rs (fee_method).
//! От чего зависит: ctx.rs, abi_check.rs, abi_v2v3.rs, families.rs, verify_rules.rs (Read).
//! Настройки: dex.v2_fee_source, dex.v2_fee_method [DEX].
//!
//! ABI и ссылки на официальные исходники — abi_check.rs, abi_v2v3.rs.

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::U24;
use alloy_primitives::{Bytes, keccak256};

use super::super::ctx::Ctx;
use super::abi_check::IPoolImmutables as P;
use super::abi_elastic::{IElasticFactory, IElasticPool};
use super::abi_v2v3::{IV2Factory, IV3Factory};
use super::families::{Family, Proto};
use super::verify_rules::Read;

/// Вызовы полей пула по виду. Ошибка — текст (не задан метод комиссии).
pub fn field_calls(ctx: &Ctx, f: &Family) -> Result<Vec<Bytes>, String> {
    let mut l = vec![
        Bytes::from(P::factoryCall {}.abi_encode()),
        Bytes::from(P::token0Call {}.abi_encode()),
        Bytes::from(P::token1Call {}.abi_encode()),
    ];
    match f.proto {
        Proto::V2 => {
            if let Some(sig) = fee_method(ctx, f)? {
                l.push(Bytes::copy_from_slice(&keccak256(sig.as_bytes())[..4]));
            }
        }
        // KyberSwap Elastic (решение 81): swapFeeUnits (uint24) и tickDistance (int24) вместо fee/tickSpacing/lmPool.
        Proto::KyberElastic => {
            l.push(Bytes::from(IElasticPool::swapFeeUnitsCall {}.abi_encode()));
            l.push(Bytes::from(IElasticPool::tickDistanceCall {}.abi_encode()));
        }
        _ => {
            l.push(Bytes::from(P::feeCall {}.abi_encode()));
            l.push(Bytes::from(P::tickSpacingCall {}.abi_encode()));
            l.push(Bytes::from(P::lmPoolCall {}.abi_encode()));
        }
    }
    Ok(l)
}

/// Метод своей комиссии пары V2: Some(сигнатура) при dex.v2_fee_source = pool, иначе None. Ошибка — не задано.
pub fn fee_method(ctx: &Ctx, f: &Family) -> Result<Option<String>, String> {
    let source = ctx
        .dex_text("dex.v2_fee_source", f.id)
        .map_err(|e| e.message)?;
    match source.as_str() {
        "pool" => Ok(Some(
            ctx.dex_text("dex.v2_fee_method", f.id)
                .map_err(|e| e.message)?
                .trim()
                .to_string(),
        )),
        _ => Ok(None),
    }
}

/// Вызов реестра фабрики: V2 getPair(token0, token1), V3 getPool(token0, token1, fee). Ошибок нет.
pub fn registry_call(f: &Family, r: &Read) -> Bytes {
    match f.proto {
        Proto::V2 => Bytes::from(
            IV2Factory::getPairCall {
                tokenA: r.token0,
                tokenB: r.token1,
            }
            .abi_encode(),
        ),
        // KyberSwap Elastic (решение 81): getPool(tokenA, tokenB, swapFeeUnits) — IFactory.sol.
        Proto::KyberElastic => Bytes::from(
            IElasticFactory::getPoolCall {
                tokenA: r.token0,
                tokenB: r.token1,
                swapFeeUnits: U24::from(r.fee.unwrap_or_default()),
            }
            .abi_encode(),
        ),
        _ => Bytes::from(
            IV3Factory::getPoolCall {
                tokenA: r.token0,
                tokenB: r.token1,
                fee: U24::from(r.fee.unwrap_or_default()),
            }
            .abi_encode(),
        ),
    }
}
