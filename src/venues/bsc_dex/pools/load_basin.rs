//! Файл: venues/bsc_dex/pools/load_basin.rs
//! Что делает: решение 485 — состояние скважины Basin на блоке: 1) слот реализации прокси ERC-1967 скважины
//! (eth_getStorageAt — прямое чтение хранилища, не ответ контракта; ноль — не прокси); 2) одна пачка try_many:
//! getReserves(), totalSupply(), wellFunction(), tokens() скважины, wellImplementation(скважина) и
//! wellImplementation(реализация из слота) Aquifer'а, getAParameter() таблиц Stable2 (dex.basin_stable2_lut). При
//! КАЖДОМ полном чтении (в том числе после Upgraded прокси — events_basin.rs) заново проверяются реализация
//! (basin_conf.rs impl_ok: клон Aquifer'а с реализацией из dex.basin_well_impls или прокси на такой клон) и функция
//! скважины по спискам настроек; не прошло — пул недостоверен (ошибка чтения с причиной). Stable2: десятичные знаки —
//! из данных функции (abi.encode(uint256, uint256), 0 → 18, больше 18 — скважина не берётся: decodeWellData откатит),
//! A — getAParameter() таблицы с тем же номером, что функция в dex.basin_stable2 (конструктор Stable2 берёт A из
//! таблицы в immutable; соответствие сверяет verify_basin_judge.rs по calcReserve). Собирает PoolState::BasinWell.
//! Кто вызывает: pools/load.rs (ветка Proto::BasinWell); calls / decode / state — ещё tests/arbitrum_basin_live.rs.
//! От чего зависит: ctx.rs, node/call.rs, node/rpc.rs, abi_basin.rs, basin_conf.rs, core/types (BasinWellData).
//! Настройки: через basin_conf.rs — dex.factory, dex.basin_* [DEX].
//!
//! Источник: BeanstalkFarms/Basin@ecf69233 src/Well.sol L106-L113 (tokens, wellFunction), L694-L705 (getReserves —
//! uint128 в слоте RESERVES_STORAGE_SLOT), src/Aquifer.sol L22 (wellImplementation), src/functions/Stable2.sol L60-L66
//! (A из LUT), L357-L374 (decodeWellData); EIP-1967 (слот реализации); eth_getStorageAt — execution-apis 16caee27
//! src/eth/state.yaml.

use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::json;

use crate::core::types::{BasinFunction, BasinWellData, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::{call, rpc};
use super::abi_basin::{IBasinAquifer as A, IBasinLookupTable as L, IBasinWell as W};
use super::basin_conf::{self, BasinConf, FnKind};

/// Вызовов скважины и Aquifer'а в пачке (дальше — по одному getAParameter() на таблицу Stable2).
const FIXED: usize = 6;

/// Ответы чтения скважины как есть.
#[derive(Debug, Clone)]
pub struct Raw {
    pub reserves: Vec<U256>,
    pub supply: U256,
    pub function: Address,
    pub function_data: Bytes,
    pub tokens: Vec<Address>,
    /// wellImplementation(скважина) Aquifer'а.
    pub registered: Address,
    /// Реализация из слота ERC-1967 скважины (ноль — не прокси) и её wellImplementation в Aquifer.
    pub slot_impl: Address,
    pub slot_registered: Address,
    /// getAParameter() таблиц Stable2 по порядку dex.basin_stable2_lut; None — таблица откатила.
    pub lut_a: Vec<Option<U256>>,
}

/// Вызовы пачки. Принимает: скважину, настройки, реализацию из слота ERC-1967 (ноль — не прокси). Возвращает: FIXED
/// вызовов и getAParameter() каждой таблицы.
pub fn calls(well: Address, conf: &BasinConf, slot_impl: Address) -> Vec<(Address, Bytes)> {
    let mut out: Vec<(Address, Bytes)> = [
        W::getReservesCall {}.abi_encode(),
        W::totalSupplyCall {}.abi_encode(),
        W::wellFunctionCall {}.abi_encode(),
        W::tokensCall {}.abi_encode(),
    ]
    .into_iter()
    .map(|d| (well, Bytes::from(d)))
    .collect();
    for w in [well, slot_impl] {
        out.push((
            conf.aquifer,
            A::wellImplementationCall { well: w }.abi_encode().into(),
        ));
    }
    for l in &conf.luts {
        out.push((*l, Bytes::from(L::getAParameterCall {}.abi_encode())));
    }
    out
}

/// Разбор ответов (без сети). Принимает: ответы по порядку calls (откат — Err), реализацию из слота. Возвращает: Raw
/// или причину.
pub fn decode(got: &[Result<Bytes, String>], slot_impl: Address) -> Result<Raw, String> {
    let at = |i: usize| match got.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!("вызов {i} скважины откатился: {r}")),
        None => Err(format!("нет ответа на вызов {i}")),
    };
    let bad = |what: &str| format!("ответ {what} скважины не по ABI");
    let reserves =
        W::getReservesCall::abi_decode_returns(at(0)?).map_err(|_| bad("getReserves"))?;
    let supply = W::totalSupplyCall::abi_decode_returns(at(1)?).map_err(|_| bad("totalSupply"))?;
    let f = W::wellFunctionCall::abi_decode_returns(at(2)?).map_err(|_| bad("wellFunction"))?;
    let tokens = W::tokensCall::abi_decode_returns(at(3)?).map_err(|_| bad("tokens"))?;
    let reg = |i: usize| {
        A::wellImplementationCall::abi_decode_returns(at(i)?)
            .map_err(|_| "ответ wellImplementation Aquifer не по ABI".to_string())
    };
    let lut_a = got
        .iter()
        .skip(FIXED)
        .map(|r| match r {
            Ok(b) => L::getAParameterCall::abi_decode_returns(b).ok(),
            Err(_) => None,
        })
        .collect();
    Ok(Raw {
        reserves,
        supply,
        function: f.target,
        function_data: f.data,
        tokens,
        registered: reg(4)?,
        slot_impl,
        slot_registered: reg(5)?,
        lut_a,
    })
}

/// Знаки токена Stable2 (decodeWellData L362-L369): 0 → 18, больше 18 — None (откат InvalidTokenDecimals).
fn dec(v: U256) -> Option<u8> {
    match u8::try_from(v).ok()? {
        0 => Some(18),
        d if d <= 18 => Some(d),
        _ => None,
    }
}

/// Состояние из ответов (без сети; его же зовут проверка и тесты на записи). Принимает: ответы, настройки, скважину.
/// Возвращает: PoolState::BasinWell или причину (реализация не допущена, не два токена / резерва, функция не из
/// списков, Stable2 без A или с данными не по ABI / знаками больше 18).
pub fn state(r: &Raw, conf: &BasinConf, well: Address) -> Result<PoolState, String> {
    basin_conf::impl_ok(conf, well, r.registered, r.slot_impl, r.slot_registered)?;
    if r.tokens.len() != 2 || r.reserves.len() != 2 {
        return Err(format!(
            "скважина из {} токенов / {} резервов — функции скважин только для двух",
            r.tokens.len(),
            r.reserves.len()
        ));
    }
    let function = match conf.kind_of(r.function)? {
        FnKind::Cp2 => BasinFunction::ConstantProduct2,
        FnKind::Stable2(k) => {
            let a = r.lut_a.get(k).copied().flatten();
            let a = a.ok_or("Stable2: нет A — таблица dex.basin_stable2_lut откатила")?;
            let (d0, d1) = <(U256, U256)>::abi_decode_params(&r.function_data)
                .map_err(|_| "Stable2: данные функции скважины не abi.encode(uint256, uint256)")?;
            let (Some(dec0), Some(dec1)) = (dec(d0), dec(d1)) else {
                return Err("Stable2: знаков токена больше 18 — обмен откатится".into());
            };
            BasinFunction::Stable2 { a, dec0, dec1 }
        }
    };
    Ok(PoolState::BasinWell(Box::new(BasinWellData {
        token0: r.tokens[0],
        token1: r.tokens[1],
        reserve0: r.reserves[0],
        reserve1: r.reserves[1],
        supply: r.supply,
        function,
    })))
}

/// Реализация из слота ERC-1967 скважины на блоке (eth_getStorageAt). Принимает: окружение, скважину, блок (тег).
/// Возвращает: адрес (ноль — не прокси). Ошибки: узел, ответ не слово.
pub async fn slot_impl(ctx: &Ctx, well: Address, tag: &str) -> GatewayResult<Address> {
    let word: B256 = rpc::call(
        ctx,
        "eth_getStorageAt",
        json!([well, basin_conf::impl_slot(), tag]),
    )
    .await?;
    Ok(Address::from_word(word))
}

/// Состояние скважины на блоке. Принимает: окружение, id DEX, скважину, блок. Ошибки: узел; NotConfigured (настройки
/// DEX — basin_conf.rs); BadResponse (откат, ответ не по ABI, реализация или функция не допущены — пул недостоверен).
pub async fn load(ctx: &Ctx, dex: &str, well: Address, block: u64) -> GatewayResult<PoolState> {
    let conf = basin_conf::read(ctx, dex).map_err(|w| ctx.err(ErrorCode::NotConfigured, w))?;
    let bad = |w: String| ctx.err(ErrorCode::BadResponse, format!("Basin {well:#x}: {w}"));
    let tag = call::block_tag(block);
    let si = slot_impl(ctx, well, &tag).await?;
    let got = call::try_many(ctx, &calls(well, &conf, si), &tag).await?;
    let raw = decode(&got, si).map_err(bad)?;
    state(&raw, &conf, well).map_err(bad)
}
