//! Файл: venues/bsc_dex/exec/executor/entry.rs
//! Что делает: вид шага исполнителя (kind.rs Params) для пула реестра (pools/registry.rs Entry) — по протоколу семейства
//! (pools/families.rs Proto): V2 и форки → V2 с комиссией пары (своя у пары или dex.v2_fee_pct DEX — как котировка,
//! quote/pool_quote.rs), PancakeSwap/Uniswap V3 и Slipstream → V3, Algebra → ALGEBRA, Solidly → SOLIDLY, DODO → DODO,
//! KyberSwap Elastic → ELASTIC. Пулы внутри менеджера-синглтона (PancakeSwap Infinity, Uniswap V4) исполнителем не
//! вызываются (у них нет своего контракта пула) — только Universal Router. Curve / WooFi / Maverick — семейств в
//! реестре пока нет (часть F): их шаги строятся прямо через kind::encode.
//! Кто вызывает: exec/executor/call.rs.
//! От чего зависит: exec/executor/kind.rs, pools/{registry, families}.rs, quote/pool_quote.rs (v2_fee_pct).
//! Настройки: через pool_quote — dex.v2_fee_pct [DEX] (если у пары нет своей комиссии).

use alloy_primitives::Address;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::ctx::Ctx;
use super::super::super::pools::families::Proto;
use super::super::super::pools::registry::Entry;
use super::super::super::quote::pool_quote;
use super::kind::Params;

/// Вид шага и адрес пула. Принимает: пул реестра. Ошибки: NoRoute — пул менеджера-синглтона (V4 / Infinity) или
/// вид без шага исполнителя; NotConfigured — комиссия V2 не задана или не в целых долях 1e-6.
pub fn params(ctx: &Ctx, e: &Entry) -> GatewayResult<(Params, Address)> {
    let pool = match &e.pool.reference {
        PoolRef::Contract(a) => *a,
        PoolRef::Singleton { .. } => {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                format!(
                    "{}: пул внутри менеджера (Infinity / V4) исполнитель не вызывает — только Universal Router",
                    e.family.id
                ),
            ));
        }
    };
    let p = match e.family.proto {
        Proto::V2 => Params::V2 {
            fee_pips: fee_pips(ctx, e)?,
        },
        Proto::PancakeV3 | Proto::UniswapV3 | Proto::Slipstream => Params::V3,
        Proto::Algebra => Params::Algebra,
        Proto::Solidly => Params::Solidly,
        Proto::DodoDvm | Proto::DodoDpp | Proto::DodoDsp => Params::Dodo,
        Proto::KyberElastic => Params::Elastic,
        Proto::InfinityCl | Proto::InfinityBin | Proto::UniswapV4 => {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                format!("{}: вид пула исполнителем не вызывается", e.family.id),
            ));
        }
    };
    Ok((p, pool))
}

/// Комиссия пары V2 в долях 1e-6: своя у пары (Pool.fee_pips) или dex.v2_fee_pct DEX × 10000. Ошибки: NotConfigured —
/// не задана или не целое число долей 1e-6 (обрезать комиссию нельзя: пара откатит обмен по проверке K).
fn fee_pips(ctx: &Ctx, e: &Entry) -> GatewayResult<u32> {
    let pct = pool_quote::v2_fee_pct(ctx, e)?;
    let pips = pct * Decimal::from(10_000u32);
    if pips.fract() != Decimal::ZERO || pips < Decimal::ZERO {
        return Err(ctx.not_set(
            "dex.v2_fee_pct",
            format!("{}: комиссия {pct}% не в целых долях 1e-6", e.family.id),
        ));
    }
    pips.to_u32().ok_or_else(|| {
        ctx.not_set(
            "dex.v2_fee_pct",
            format!("{}: комиссия {pct}% вне диапазона", e.family.id),
        )
    })
}
