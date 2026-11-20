//! Файл: venues/bsc_dex/quote/camelot_quote.rs
//! Что делает: решение 443 (T433-F4) — обмен в паре Camelot V2 по локальному состоянию (PoolState::Solidly с
//! направленной комиссией `fee` / `fee1`): точный вход — как getAmountOut пары и проверки swap (math/camelot_v2.rs);
//! точный выход — только у волатильной пары: обратная формула V2 с комиссией входа (FEE_DENOMINATOR − fee) /
//! FEE_DENOMINATOR — та же дробь, что у прямой формулы пары (Pair.sol L391-L392), getAmountIn у пары нет; стабильная
//! пара точный выход не считает (обратной формулы кривой нет).
//! Кто вызывает: quote/pool_quote.rs (ветка Solidly при Proto::CamelotV2).
//! От чего зависит: math/camelot_v2.rs, math/v2.rs, math/swap_cl.rs (Swapped), core/types (SolidlyFee).
//! Настройки: нет (всё — с контракта пары, pools/load_camelot.rs).

use alloy_primitives::U256;

use crate::core::types::SolidlyFee;

use super::super::math::camelot_v2::{self, FEE_DENOMINATOR, Pair};
use super::super::math::swap_cl::Swapped;
use super::super::math::v2;

/// Резервы, флаг stable и множители точности пары (10^decimals) — поля PoolState::Solidly.
pub type State = (U256, U256, bool, U256, U256);

/// Обмен в паре. Принимает: состояние, комиссию входа token0 (`fee`) и token1 (`fee1`; у пары Camelot — всегда есть),
/// (направление: вход token0, точный вход, сумма). Возвращает: Swapped. Ошибка — текст причины (нет fee1 — запись не
/// пары Camelot; обмен откатился бы; точный выход стабильной пары).
pub fn swap(
    (reserve0, reserve1, stable, pm0, pm1): State,
    fee: &SolidlyFee,
    fee1: Option<&SolidlyFee>,
    (zero_for_one, exact_in, amount): (bool, bool, U256),
) -> Result<Swapped, String> {
    let Some(fee1) = fee1 else {
        return Err("Camelot V2: нет комиссии входа token1".into());
    };
    let p = Pair {
        reserve0,
        reserve1,
        stable,
        pm0,
        pm1,
        fee0: U256::from(fee.value),
        fee1: U256::from(fee1.value),
    };
    if exact_in {
        let out = camelot_v2::swap_out(&p, amount, zero_for_one)
            .ok_or("Camelot V2: обмен откатился бы (как в контракте пары)")?;
        return Ok(Swapped {
            amount_in: amount,
            amount_out: out,
            complete: true,
        });
    }
    if stable {
        return Err("Camelot V2 stable: точного выхода нет (нет обратной формулы)".into());
    }
    let (r_in, r_out, f) = if zero_for_one {
        (reserve0, reserve1, p.fee0)
    } else {
        (reserve1, reserve0, p.fee1)
    };
    let den = U256::from(FEE_DENOMINATOR);
    let keep = den
        .checked_sub(f)
        .ok_or("Camelot V2: комиссия больше FEE_DENOMINATOR")?;
    let amount_in = v2::amount_in(amount, r_in, r_out, (keep, den))
        .ok_or("Camelot V2: в паре нет такого выхода")?;
    Ok(Swapped {
        amount_in,
        amount_out: amount,
        complete: true,
    })
}
