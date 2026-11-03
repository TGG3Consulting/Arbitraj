//! Файл: venues/bsc_dex/exec/executor/entry.rs
//! Что делает: вид шага исполнителя (kind.rs Params) для пула реестра (pools/registry.rs Entry) по протоколу семейства
//! (pools/families.rs Proto) и ОТМЕТКА ГОТОВНОСТИ семейства к исполнителю (step_kind) — по ней же исполнимость графа
//! (quote/graph_exec.rs) и выбор пути (exec/executor/mod.rs::takes). Готовы: V2 и форки V2 → V2 с комиссией пары (своя у
//! пары или dex.v2_fee_pct DEX — как котировка, quote/pool_quote.rs), PancakeSwap/Uniswap V3 → V3. Slipstream (→ V3),
//! Algebra, Solidly, DODO — виды в контракте есть, семейство включает часть F (одна строка в step_kind) после живой
//! проверки. KyberSwap Elastic — вид ELASTIC в контракте есть, но семейство неисполнимо (решение 121 B2). Пулы внутри
//! менеджера-синглтона (PancakeSwap Infinity, Uniswap V4) — только Universal Router. Curve / WooFi / Maverick —
//! семейств в реестре пока нет (часть F).
//! Кто вызывает: exec/executor/{call, mod}.rs, quote/graph_exec.rs.
//! От чего зависит: exec/executor/kind.rs, pools/{registry, families}.rs, quote/pool_quote.rs (v2_fee_pct).
//! Настройки: через pool_quote — dex.v2_fee_pct [DEX] (если у пары нет своей комиссии).

use alloy_primitives::Address;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::ctx::Ctx;
use super::super::super::pools::families::{Family, Proto};
use super::super::super::pools::registry::Entry;
use super::super::super::quote::pool_quote;
use super::kind::Params;

/// Вид шага без параметров пула (у V2 комиссия — из пула, см. params).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Base {
    V2,
    Ready(Params),
}

/// Отметка готовности: протокол семейства → вид шага или причина, почему через исполнитель не идёт. Ошибок иных нет.
fn step_kind(proto: Proto) -> Result<Base, &'static str> {
    match proto {
        Proto::V2 => Ok(Base::V2),
        Proto::PancakeV3 | Proto::UniswapV3 => Ok(Base::Ready(Params::V3)),
        Proto::KyberElastic => Err("KyberSwap Elastic — только сигнал (решение 121 B2)"),
        Proto::Slipstream
        | Proto::Algebra
        | Proto::Solidly
        | Proto::DodoDvm
        | Proto::DodoDpp
        | Proto::DodoDsp => Err("семейство ещё не включено к исполнителю (часть F)"),
        Proto::InfinityCl | Proto::InfinityBin | Proto::UniswapV4 => Err(
            "пул внутри менеджера (Infinity / V4) исполнитель не вызывает — только Universal Router",
        ),
    }
}

/// Может ли шаг через этот пул идти исполнителем: у пула свой контракт и семейство отмечено готовым. Принимает:
/// семейство и ссылку на пул (хватает хопа маршрута). Возвращает: адрес пула или причину. Ошибок иных нет.
pub fn supported(f: &Family, pool: &PoolRef) -> Result<Address, String> {
    step_kind(f.proto).map_err(|w| format!("{}: {w}", f.id))?;
    match pool {
        PoolRef::Contract(a) => Ok(*a),
        PoolRef::Singleton { .. } => Err(format!(
            "{}: пул внутри менеджера исполнитель не вызывает",
            f.id
        )),
    }
}

/// Вид шага и адрес пула. Принимает: пул реестра. Ошибки: NoRoute — семейство не готово к исполнителю или пул
/// менеджера-синглтона (supported); NotConfigured — комиссия V2 не задана или не в целых долях 1e-6.
pub fn params(ctx: &Ctx, e: &Entry) -> GatewayResult<(Params, Address)> {
    let pool =
        supported(e.family, &e.pool.reference).map_err(|w| ctx.err(ErrorCode::NoRoute, w))?;
    let p = match step_kind(e.family.proto) {
        Ok(Base::V2) => Params::V2 {
            fee_pips: fee_pips(ctx, e)?,
        },
        Ok(Base::Ready(p)) => p,
        Err(w) => return Err(ctx.err(ErrorCode::NoRoute, w)),
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
