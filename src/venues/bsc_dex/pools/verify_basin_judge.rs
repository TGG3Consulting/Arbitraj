//! Файл: venues/bsc_dex/pools/verify_basin_judge.rs
//! Что делает: решение 485 — решения проверки скважины Basin по ответам узла (без сети; их же зовут тесты на записи):
//! - первый проход (first_calls / first): Aquifer.wellImplementation(скважина), getImplementation() скважины (есть только
//!   у WellUpgradeable за прокси ERC-1967; откат — не прокси), aquifer(), tokens(), wellFunction(), getReserves();
//!   скважина Aquifer'а dex.factory, два токена = монета и встречный, функция из списков;
//! - второй проход (second_calls / second): у прокси — Aquifer.wellImplementation(реализация прокси) из
//!   dex.basin_well_impls (скважины Beanstalk на Arbitrum — ERC1967Proxy на клон WellUpgradeable, выпущенный Aquifer:
//!   BeanstalkFarms/Beanstalk@6db8c07c ReseedBean.sol L225-L257); функция скважины calcLpTokenSupply(резервы, данные)
//!   = наш расчёт (math/basin_well.rs) — сверка версии функции и, у Stable2, A из dex.basin_stable2_lut.
//!
//! Кто вызывает: pools/verify_basin.rs.
//! От чего зависит: abi_basin.rs, basin_conf.rs, load_basin.rs (state), math/basin_well.rs, discover.rs.
//! Настройки: нет (настройки — BasinConf).
//!
//! Источник: BeanstalkFarms/Basin@ecf69233 src/Aquifer.sol L22, L79; src/WellUpgradeable.sol L132-L134; src/Well.sol
//! L106-L158, L694-L696.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{BasinWellData, PoolState};

use super::super::math::basin_well;
use super::abi_basin::{
    IBasinAquifer as A, IBasinLookupTable as L, IBasinWell as W, IBasinWellFunction as F,
};
use super::basin_conf::BasinConf;
use super::discover::sorted;
use super::load_basin::{self, Raw};

/// Вызовов первого прохода на скважину без таблицы Stable2 (с таблицей — на один больше: getAParameter()).
pub const FIRST: usize = 6;

/// Итог первого прохода: состояние скважины и реализация прокси, которую надо проверить вторым проходом (None —
/// скважина сама выпущена Aquifer'ом с допущенной реализацией).
#[derive(Debug, Clone)]
pub struct First {
    pub data: BasinWellData,
    pub function: Address,
    pub function_data: Bytes,
    pub proxy_impl: Option<Address>,
}

/// Вызовы первого прохода. Принимает: скважину, Aquifer, таблицу Stable2 (None — не задана). Возвращает: FIRST
/// вызовов по порядку и getAParameter() таблицы последним, если она задана.
pub fn first_calls(well: Address, aquifer: Address, lut: Option<Address>) -> Vec<(Address, Bytes)> {
    let mut out = vec![
        (aquifer, A::wellImplementationCall { well }.abi_encode().into()),
        (well, W::getImplementationCall {}.abi_encode().into()),
        (well, W::aquiferCall {}.abi_encode().into()),
        (well, W::tokensCall {}.abi_encode().into()),
        (well, W::wellFunctionCall {}.abi_encode().into()),
        (well, W::getReservesCall {}.abi_encode().into()),
    ];
    if let Some(l) = lut {
        out.push((l, L::getAParameterCall {}.abi_encode().into()));
    }
    out
}

/// Первый проход. Принимает: скважину, монету и встречный кандидата, настройки, ответы first_calls (откат — Err).
/// Возвращает: First или причину отказа.
pub fn first(
    well: Address,
    pair: (Address, Address),
    conf: &BasinConf,
    a: &[Result<Bytes, String>],
) -> Result<First, String> {
    let ok = |i: usize, what: &str| match a.get(i) {
        Some(Ok(b)) => Ok(b.clone()),
        Some(Err(r)) => Err(format!("скважина {well:#x}: {what} откатился: {r}")),
        None => Err(format!("скважина {well:#x}: нет ответа {what}")),
    };
    let abi = |what: &str| format!("скважина {well:#x}: ответ {what} не по ABI");
    let registered = A::wellImplementationCall::abi_decode_returns(&ok(0, "wellImplementation")?)
        .map_err(|_| abi("wellImplementation"))?;
    let aq = W::aquiferCall::abi_decode_returns(&ok(2, "aquifer()")?)
        .map_err(|_| abi("aquifer()"))?;
    if aq != conf.aquifer {
        return Err(format!("aquifer() скважины {aq:#x} ≠ dex.factory {:#x}", conf.aquifer));
    }
    let proxy_impl = if conf.impls.contains(&registered) {
        None
    } else {
        // Не клон Aquifer'а с допущенной реализацией — тогда только прокси ERC-1967 на такой клон.
        let imp = match a.get(1) {
            Some(Ok(b)) => W::getImplementationCall::abi_decode_returns(b).ok(),
            _ => None,
        };
        match imp {
            Some(i) if !i.is_zero() => Some(i),
            _ => {
                return Err(format!(
                    "скважина {well:#x}: реализация в Aquifer {registered:#x} не из dex.basin_well_impls, и это не \
                     прокси скважины (getImplementation)"
                ));
            }
        }
    };
    let f = W::wellFunctionCall::abi_decode_returns(&ok(4, "wellFunction()")?)
        .map_err(|_| abi("wellFunction()"))?;
    // Число LP-токенов проверке не нужно (сверка — calcLpTokenSupply по резервам): supply = 0 только здесь.
    let raw = Raw {
        reserves: W::getReservesCall::abi_decode_returns(&ok(5, "getReserves")?)
            .map_err(|_| abi("getReserves"))?,
        supply: U256::ZERO,
        function: f.target,
        function_data: f.data.clone(),
        tokens: W::tokensCall::abi_decode_returns(&ok(3, "tokens()")?)
            .map_err(|_| abi("tokens()"))?,
        lut_a: match a.get(FIRST) {
            Some(Ok(b)) => L::getAParameterCall::abi_decode_returns(b).ok(),
            _ => None,
        },
    };
    let PoolState::BasinWell(d) = load_basin::state(&raw, conf)? else {
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
        function: f.target,
        function_data: f.data,
        proxy_impl,
    })
}

/// Вызовы второго прохода: [wellImplementation(реализация прокси)], calcLpTokenSupply(резервы, данные) функции.
/// Принимает: итог первого прохода, настройки. Возвращает: вызовы по порядку.
pub fn second_calls(f: &First, conf: &BasinConf) -> Vec<(Address, Bytes)> {
    let mut out = Vec::new();
    if let Some(i) = f.proxy_impl {
        out.push((conf.aquifer, A::wellImplementationCall { well: i }.abi_encode().into()));
    }
    let reserves = vec![f.data.reserve0, f.data.reserve1];
    let c = F::calcLpTokenSupplyCall {
        reserves,
        data: f.function_data.clone(),
    };
    out.push((f.function, c.abi_encode().into()));
    out
}

/// Второй проход. Принимает: итог первого, настройки, ответы second_calls. Возвращает: Ok — скважина берётся; Err —
/// причина (реализация прокси не допущена; calcLpTokenSupply функции ≠ нашему расчёту — другая версия функции или,
/// у Stable2, другая таблица A).
pub fn second(f: &First, conf: &BasinConf, a: &[Result<Bytes, String>]) -> Result<(), String> {
    let mut k = 0;
    let mut next = || {
        k += 1;
        match a.get(k - 1) {
            Some(Ok(b)) => Ok(b.clone()),
            Some(Err(r)) => Err(format!("второй проход проверки скважины: откат {r}")),
            None => Err("второй проход проверки скважины: нет ответа".to_string()),
        }
    };
    if let Some(i) = f.proxy_impl {
        let reg = A::wellImplementationCall::abi_decode_returns(&next()?)
            .map_err(|_| "wellImplementation не по ABI".to_string())?;
        if !conf.impls.contains(&reg) {
            return Err(format!(
                "реализация прокси {i:#x}: в Aquifer — {reg:#x}, не из dex.basin_well_impls"
            ));
        }
    }
    let theirs = F::calcLpTokenSupplyCall::abi_decode_returns(&next()?)
        .map_err(|_| "calcLpTokenSupply не по ABI".to_string())?;
    let ours = basin_well::lp_supply(&f.data);
    if ours != Some(theirs) {
        return Err(format!(
            "calcLpTokenSupply функции {:#x} = {theirs}, наш расчёт — {ours:?}: функция другой версии или A таблицы \
             dex.basin_stable2_lut не её",
            f.function
        ));
    }
    Ok(())
}
