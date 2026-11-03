//! Файл: venues/bsc_dex/exec/executor/entry.rs
//! Что делает: шаг исполнителя (kind.rs Params) для пула реестра (pools/registry.rs Entry). Вид шага — пометка части F на
//! семействе (Family.executor_kind, pools/exec_kind.rs): V2 → V2 с комиссией пары (своя у пары или dex.v2_fee_pct DEX —
//! как котировка, quote/pool_quote.rs), V3 (PancakeSwap/Uniswap V3, форки, Slipstream) → V3, ALGEBRA, SOLIDLY, DODO.
//! Нет вида шага — пул исполнителем не вызывается (Infinity / V4 — только Universal Router; KyberSwap Elastic — вид
//! ELASTIC в контракте есть, но семейство неисполнимо, решение 121 B2). CURVE / WOO / MAVERICK из реестра пока не
//! приходят (семейств нет); у CURVE нужны индексы монет пула — без них шаг не строится.
//! Кто вызывает: exec/executor/{call, mod}.rs, quote/graph_exec.rs.
//! От чего зависит: exec/executor/kind.rs, pools/{registry, families, exec_kind}.rs, quote/pool_quote.rs (v2_fee_pct).
//! Настройки: через pool_quote — dex.v2_fee_pct [DEX] (если у пары нет своей комиссии).

use alloy_primitives::Address;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::ctx::Ctx;
use super::super::super::pools::exec_kind::ExecKind;
use super::super::super::pools::families::Family;
use super::super::super::pools::registry::Entry;
use super::super::super::quote::pool_quote;
use super::kind::Params;

/// Может ли шаг через этот пул идти исполнителем: у семейства есть вид шага (Family.executor_kind) и у пула свой
/// контракт. Принимает: семейство и ссылку на пул (хватает хопа маршрута). Возвращает: вид шага и адрес пула или
/// причину. Ошибок иных нет.
pub fn supported(f: &Family, pool: &PoolRef) -> Result<(ExecKind, Address), String> {
    let k = f
        .executor_kind
        .ok_or_else(|| format!("{}: у вида пула нет шага исполнителя", f.id))?;
    match pool {
        PoolRef::Contract(a) => Ok((k, *a)),
        PoolRef::Singleton { .. } => Err(format!(
            "{}: пул внутри менеджера исполнитель не вызывает",
            f.id
        )),
    }
}

/// Вид шага и адрес пула. Принимает: пул реестра. Ошибки: NoRoute — у семейства нет шага исполнителя, пул
/// менеджера-синглтона или шаг вида без параметров из реестра (CURVE); NotConfigured — комиссия V2 не задана или не в
/// целых долях 1e-6.
pub fn params(ctx: &Ctx, e: &Entry) -> GatewayResult<(Params, Address)> {
    let (k, pool) =
        supported(e.family, &e.pool.reference).map_err(|w| ctx.err(ErrorCode::NoRoute, w))?;
    let p = match k {
        ExecKind::V2 => Params::V2 {
            fee_pips: fee_pips(ctx, e)?,
        },
        ExecKind::V3 => Params::V3,
        ExecKind::Algebra => Params::Algebra,
        ExecKind::Solidly => Params::Solidly,
        ExecKind::Dodo => Params::Dodo,
        ExecKind::Woo => Params::Woo,
        ExecKind::Maverick => Params::Maverick,
        ExecKind::Curve => {
            let why = format!("{}: индексы монет Curve в реестре не известны", e.family.id);
            return Err(ctx.err(ErrorCode::NoRoute, why));
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
