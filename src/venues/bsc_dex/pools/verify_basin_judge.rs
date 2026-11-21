//! Файл: venues/bsc_dex/pools/verify_basin_judge.rs
//! Что делает: решение 485 — решения проверки скважины Basin по ответам узла (без сети; их же зовут тесты на записи):
//! - первый проход (first): те же ответы, что у чтения бота (load_basin.rs: слот ERC-1967, пачка calls) — состояние
//!   собирается (реализация допущена — basin_conf.rs impl_ok, функция из списков), два токена = монета и встречный;
//! - второй проход (second_calls / second): calcReserve функции скважины на СДВИНУТЫХ резервах = наш расчёт
//!   (math/basin_well.rs) — сверка версии функции и, у Stable2, A таблицы dex.basin_stable2_lut.
//!
//! Почему calcReserve, а не calcLpTokenSupply на текущих резервах (ревью t254): при равных приведённых резервах
//! инвариант Stable2 даёт D = b0 + b1 при любом A (точка равновесия лежит на кривой каждого A) — такая сверка A не
//! доказывает. calcReserve(резервы', j = 1, D) зависит от A, если приведённый резерв 0 не равен D/2; сдвиг уводит его
//! от D/2: при 2·x0 ≥ D — x0 + x0/2, иначе — x0/2 (расстояние до D/2 растёт на x0/2). Ровно D/2, пустая скважина
//! (D = 0 или x0 = 0) — «не сверено», скважина не берётся до другого состояния.
//!
//! Кто вызывает: pools/verify_basin.rs; tests/arbitrum_basin_live.rs.
//! От чего зависит: abi_basin.rs, basin_conf.rs, load_basin.rs (state), math/basin_well.rs, discover.rs.
//! Настройки: нет (настройки — BasinConf).
//!
//! Источник: BeanstalkFarms/Basin@ecf69233 src/functions/Stable2.sol L132-L162 (calcReserve), L419-L426 (масштаб),
//! src/functions/ConstantProduct2.sol L62-L74.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{BasinFunction, BasinWellData, PoolState};

use super::super::math::basin_well;
use super::abi_basin::IBasinWellFunction as F;
use super::basin_conf::BasinConf;
use super::discover::sorted;
use super::load_basin::{self, Raw};

/// Итог первого прохода: состояние скважины, функция и её данные.
#[derive(Debug, Clone)]
pub struct First {
    pub data: BasinWellData,
    pub function: Address,
    pub function_data: Bytes,
}

/// Первый проход. Принимает: скважину, монету и встречный кандидата, настройки, разобранные ответы чтения.
/// Возвращает: First или причину отказа.
pub fn first(
    well: Address,
    pair: (Address, Address),
    conf: &BasinConf,
    raw: &Raw,
) -> Result<First, String> {
    let PoolState::BasinWell(d) = load_basin::state(raw, conf, well)? else {
        return Err("состояние скважины не BasinWell".into());
    };
    if d.token0 == d.token1 || sorted(d.token0, d.token1) != sorted(pair.0, pair.1) {
        return Err(format!(
            "токены скважины {:#x} / {:#x} ≠ монета {:#x} и встречный {:#x}",
            d.token0, d.token1, pair.0, pair.1
        ));
    }
    Ok(First {
        data: *d,
        function: raw.function,
        function_data: raw.function_data.clone(),
    })
}

/// 10^(18 − знаки) резерва 0 (Stable2 getScaledReserves); ConstantProduct2 — 1. None — знаков больше 18.
fn scale0(d: &BasinWellData) -> Option<U256> {
    match d.function {
        BasinFunction::ConstantProduct2 => Some(U256::from(1u8)),
        BasinFunction::Stable2 { dec0, .. } => {
            let e = 18u8.checked_sub(dec0)?;
            Some(U256::from(10u8).pow(U256::from(e)))
        }
    }
}

/// Сдвинутые резервы сверки (см. шапку). Возвращает: [резерв 0', резерв 1] или причину «не сверено».
pub fn shifted(d: &BasinWellData) -> Result<[U256; 2], String> {
    let nope = |why: &str| format!("сверка функции скважины не сделана: {why} — не сверено");
    if d.supply.is_zero() || d.reserve0.is_zero() {
        return Err(nope("скважина пуста"));
    }
    let k = scale0(d).ok_or_else(|| nope("знаков больше 18"))?;
    let x0 = d
        .reserve0
        .checked_mul(k)
        .ok_or_else(|| nope("переполнение"))?;
    let two = U256::from(2u8);
    let r0 = if x0.checked_mul(two).is_none_or(|v| v >= d.supply) {
        d.reserve0 + d.reserve0 / two
    } else {
        d.reserve0 / two
    };
    let x = r0.checked_mul(k).and_then(|v| v.checked_mul(two));
    if r0.is_zero() || x == Some(d.supply) {
        return Err(nope("резерв в точке равновесия"));
    }
    Ok([r0, d.reserve1])
}

/// Вызов второго прохода: calcReserve(сдвинутые резервы, 1, totalSupply, данные) функции. Принимает: итог первого
/// прохода. Возвращает: вызов или причину «не сверено».
pub fn second_calls(f: &First) -> Result<Vec<(Address, Bytes)>, String> {
    let r = shifted(&f.data)?;
    let c = F::calcReserveCall {
        reserves: r.to_vec(),
        j: U256::from(1u8),
        lpTokenSupply: f.data.supply,
        data: f.function_data.clone(),
    };
    Ok(vec![(f.function, c.abi_encode().into())])
}

/// Второй проход. Принимает: итог первого, ответы second_calls. Возвращает: Ok — скважина берётся; Err — причина
/// (calcReserve функции ≠ нашему расчёту: функция другой версии или, у Stable2, таблица A не её; откат функции при
/// нашем «расчёт есть» и наоборот).
pub fn second(f: &First, a: &[Result<Bytes, String>]) -> Result<(), String> {
    let r = shifted(&f.data)?;
    let ours = basin_well::calc_reserve(&f.data, r, 1, f.data.supply);
    let theirs = match a.first() {
        Some(Ok(b)) => Some(
            F::calcReserveCall::abi_decode_returns(b)
                .map_err(|_| "calcReserve не по ABI".to_string())?,
        ),
        Some(Err(_)) => None,
        None => return Err("второй проход проверки скважины: нет ответа".into()),
    };
    if ours != theirs {
        return Err(format!(
            "calcReserve функции {:#x} = {theirs:?}, наш расчёт — {ours:?}: функция другой \
             версии или A таблицы dex.basin_stable2_lut не её",
            f.function
        ));
    }
    Ok(())
}
