//! Файл: venues/bsc_dex/exec/executor/entry.rs
//! Что делает: шаг исполнителя (kind.rs Params) для пула реестра (pools/registry.rs Entry). Вид шага — пометка части F на
//! семействе (Family.executor_kind, pools/exec_kind.rs): V2 → V2 с комиссией пары (своя у пары или dex.v2_fee_pct DEX —
//! как котировка, quote/pool_quote.rs), V3 (PancakeSwap/Uniswap V3, форки, Slipstream) → V3, ALGEBRA, SOLIDLY, DODO.
//! Нет вида шага — пул исполнителем не вызывается (Infinity / V4 — только Universal Router; KyberSwap Elastic — вид
//! ELASTIC в контракте есть, но семейство неисполнимо, решение 121 B2). CURVE / WOO / MAVERICK из реестра пока не
//! приходят (семейств нет). CURVE (StableSwap Ellipsis, решение 122 F2) — индексы пары из состояния пула
//! (PoolState::StableSwap: i0 — token0, i1 — token1), направление — по токену входа шага; так же STABLE_U256
//! (PancakeSwap Stable, Lista) и SADDLE (Nerve) — решение 122 X, стейблсвапы; пул с halted — не исполняется.
//! Монета пары — нативный BNB пула (StableData.native, в паре — обёртка WBNB; решение «122 X (нативный BNB в
//! стейблсвапах)»): вид STABLE_U256_BNB (bnb_in — по тому, вход это или выход); у CURVE / SADDLE — отказ.
//! Кто вызывает: exec/executor/{call, mod}.rs, quote/graph_exec.rs.
//! От чего зависит: exec/executor/kind.rs, pools/{registry, families, exec_kind}.rs, quote/pool_quote.rs (v2_fee_pct).
//! Настройки: через pool_quote — dex.v2_fee_pct [DEX] (если у пары нет своей комиссии).

use alloy_primitives::Address;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::core::types::{PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::ctx::Ctx;
use super::super::super::pools::exec_kind::ExecKind;
use super::super::super::pools::families::Family;
use super::super::super::pools::registry::Entry;
use super::super::super::quote::pool_quote;
use super::kind::{Params, STABLE_NATIVE};

/// Может ли шаг через этот пул идти исполнителем: у семейства есть вид шага (Family.executor_kind) и у пула свой
/// контракт. Принимает: семейство и ссылку на пул (хватает хопа маршрута). Возвращает: вид шага и адрес пула или
/// причину. Ошибок иных нет.
pub fn supported(f: &Family, pool: &PoolRef) -> Result<(ExecKind, Address), String> {
    let k = f
        .executor_kind
        .ok_or_else(|| format!("{}: у вида пула нет шага исполнителя", f.id))?;
    match pool {
        PoolRef::Contract(a) => Ok((k, *a)),
        // Пара общего контракта (WooPPV2, решение 122 F, 3): исполнитель вызывает сам общий контракт (шаг WOO).
        PoolRef::Member { contract, .. } => Ok((k, *contract)),
        PoolRef::Singleton { .. } => Err(format!(
            "{}: пул внутри менеджера исполнитель не вызывает",
            f.id
        )),
    }
}

/// Вид шага и адрес пула. Принимает: пул реестра. Ошибки: NoRoute — у семейства нет шага исполнителя, пул
/// менеджера-синглтона, у CURVE нет состояния StableSwap или токен входа не из пары пула; NotConfigured — комиссия V2
/// не задана или не в целых долях 1e-6. `token_in` — токен входа шага (нужен виду CURVE для порядка i → j).
pub fn params(ctx: &Ctx, e: &Entry, token_in: Address) -> GatewayResult<(Params, Address)> {
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
        ExecKind::Curve | ExecKind::StableU256 | ExecKind::Saddle => {
            stable(e, k, token_in).map_err(|w| ctx.err(ErrorCode::NoRoute, w))?
        }
    };
    Ok((p, pool))
}

/// Индексы шага StableSwap (CURVE, STABLE_U256, SADDLE): i — монета входа, j — выхода. Принимает: пул реестра, вид,
/// токен входа. Возвращает: параметры шага или причину (состояние не StableSwap; обмены пула остановлены — пауза /
/// kill / сбой проверки цены Lista; в паре — сама метка 0xEeee…; нативная монета пары у вида без шага с BNB; токен входа
/// не token0/token1 пула).
fn stable(e: &Entry, k: ExecKind, token_in: Address) -> Result<Params, String> {
    let PoolState::StableSwap(d) = &e.pool.state else {
        return Err(format!("{}: у пула нет состояния StableSwap", e.family.id));
    };
    if d.halted {
        return Err(format!("{}: обмены пула остановлены", e.family.id));
    }
    // Метка BNB_ADDRESS (0xEeee…) в паре быть не должна: загрузка ставит вместо неё обёртку (pools/stable_native.rs);
    // токеном шага она быть не может — отказ до отправки.
    if [e.pool.token0, e.pool.token1].contains(&STABLE_NATIVE) {
        return Err(format!(
            "{}: в паре метка нативной монеты 0xEeee… (не ERC-20) — шаг не строится",
            e.family.id
        ));
    }
    let (i, j) = if token_in == e.pool.token0 {
        (d.i0, d.i1)
    } else if token_in == e.pool.token1 {
        (d.i1, d.i0)
    } else {
        return Err(format!(
            "{}: токен входа {token_in} не из пары пула",
            e.family.id
        ));
    };
    // Нативная монета пула — монета этой пары (вход или выход шага): шаг с BNB (только PancakeSwap Stable / Lista).
    if let Some(n) = d.native.filter(|n| *n == i || *n == j) {
        return match k {
            ExecKind::StableU256 => Ok(Params::StableU256Bnb {
                i,
                j,
                bnb_in: n == i,
            }),
            _ => Err(format!(
                "{}: монета пары — нативный BNB пула, у вида {} шага с BNB нет",
                e.family.id,
                k.name()
            )),
        };
    }
    Ok(match k {
        ExecKind::StableU256 => Params::StableU256 { i, j },
        ExecKind::Saddle => Params::Saddle { i, j },
        _ => Params::Curve {
            i: i128::from(i),
            j: i128::from(j),
        },
    })
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
