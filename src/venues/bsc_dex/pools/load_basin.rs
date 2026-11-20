//! Файл: venues/bsc_dex/pools/load_basin.rs
//! Что делает: решение 485 — состояние скважины Basin на блоке одной пачкой (try_many): getReserves(), totalSupply(),
//! wellFunction(), tokens() скважины и getAParameter() таблицы Stable2 (dex.basin_stable2_lut, если задана). Функция
//! скважины проверяется по спискам настроек при КАЖДОМ чтении (прокси скважины Beanstalk можно перевести на другую
//! реализацию — Upgraded, events_basin.rs). Stable2: десятичные знаки — из данных функции (abi.encode(uint256,
//! uint256), 0 → 18, больше 18 — скважина не берётся: decodeWellData откатит), A — getAParameter() таблицы (его же
//! конструктор Stable2 берёт в immutable `a`; соответствие таблицы функции сверяет verify_basin.rs по
//! calcLpTokenSupply). Собирает PoolState::BasinWell.
//! Кто вызывает: pools/load.rs (ветка Proto::BasinWell); state — ещё tests/arbitrum_basin_live.rs.
//! От чего зависит: ctx.rs, node/call.rs, abi_basin.rs, basin_conf.rs, core/types (BasinWellData).
//! Настройки: через basin_conf.rs — dex.basin_cp2, dex.basin_stable2, dex.basin_stable2_lut [DEX].
//!
//! Источник: BeanstalkFarms/Basin@ecf69233 src/Well.sol L106-L113 (tokens, wellFunction), L694-L705 (getReserves —
//! uint128 в слоте RESERVES_STORAGE_SLOT), src/functions/Stable2.sol L60-L66 (A из LUT), L357-L374 (decodeWellData).

use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{BasinFunction, BasinWellData, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_basin::{IBasinLookupTable as L, IBasinWell as W};
use super::basin_conf::{self, BasinConf, FnKind};

/// Ответы чтения скважины как есть (по порядку calls).
#[derive(Debug, Clone)]
pub struct Raw {
    pub reserves: Vec<U256>,
    pub supply: U256,
    pub function: Address,
    pub function_data: Bytes,
    pub tokens: Vec<Address>,
    /// getAParameter() таблицы Stable2; None — таблица не задана или откатила.
    pub lut_a: Option<U256>,
}

/// Вызовы чтения. Принимает: скважину, таблицу Stable2 (None — не задана). Возвращает: (адрес, данные) по порядку.
pub fn calls(well: Address, lut: Option<Address>) -> Vec<(Address, Bytes)> {
    let mut out: Vec<(Address, Bytes)> = [
        W::getReservesCall {}.abi_encode(),
        W::totalSupplyCall {}.abi_encode(),
        W::wellFunctionCall {}.abi_encode(),
        W::tokensCall {}.abi_encode(),
    ]
    .into_iter()
    .map(|d| (well, Bytes::from(d)))
    .collect();
    if let Some(l) = lut {
        out.push((l, Bytes::from(L::getAParameterCall {}.abi_encode())));
    }
    out
}

/// Разбор ответов (без сети). Принимает: ответы по порядку calls (откат — Err). Возвращает: Raw или причину.
pub fn decode(got: &[Result<Bytes, String>]) -> Result<Raw, String> {
    let at = |i: usize| match got.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!("вызов {i} скважины откатился: {r}")),
        None => Err(format!("нет ответа на вызов {i}")),
    };
    let bad = |what: &str| format!("ответ {what} скважины не по ABI");
    let reserves = W::getReservesCall::abi_decode_returns(at(0)?).map_err(|_| bad("getReserves"))?;
    let supply = W::totalSupplyCall::abi_decode_returns(at(1)?).map_err(|_| bad("totalSupply"))?;
    let f = W::wellFunctionCall::abi_decode_returns(at(2)?).map_err(|_| bad("wellFunction"))?;
    let tokens = W::tokensCall::abi_decode_returns(at(3)?).map_err(|_| bad("tokens"))?;
    let lut_a = match got.get(4) {
        Some(Ok(b)) => L::getAParameterCall::abi_decode_returns(b).ok(),
        _ => None,
    };
    Ok(Raw {
        reserves,
        supply,
        function: f.target,
        function_data: f.data,
        tokens,
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

/// Состояние из ответов (без сети; его же зовут тесты на записи). Принимает: ответы, настройки. Возвращает:
/// PoolState::BasinWell или причину (не два токена / резерва, функция не из списков, Stable2 без таблицы или с
/// данными не по ABI / знаками больше 18).
pub fn state(r: &Raw, conf: &BasinConf) -> Result<PoolState, String> {
    if r.tokens.len() != 2 || r.reserves.len() != 2 {
        return Err(format!(
            "скважина из {} токенов / {} резервов — функции скважин только для двух",
            r.tokens.len(),
            r.reserves.len()
        ));
    }
    let function = match conf.kind_of(r.function)? {
        FnKind::Cp2 => BasinFunction::ConstantProduct2,
        FnKind::Stable2 => {
            let a = r.lut_a.ok_or("Stable2: нет A — dex.basin_stable2_lut не задана или откатила")?;
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

/// Состояние скважины на блоке. Принимает: окружение, id DEX, скважину, блок. Ошибки: узел; NotConfigured (настройки
/// DEX — basin_conf.rs); BadResponse (откат, ответ не по ABI, функция не допущена).
pub async fn load(ctx: &Ctx, dex: &str, well: Address, block: u64) -> GatewayResult<PoolState> {
    let conf = basin_conf::read(ctx, dex).map_err(|w| ctx.err(ErrorCode::NotConfigured, w))?;
    let bad = |w: String| ctx.err(ErrorCode::BadResponse, format!("Basin {well:#x}: {w}"));
    let c = calls(well, conf.lut);
    let got = call::try_many(ctx, &c, &call::block_tag(block)).await?;
    let raw = decode(&got).map_err(bad)?;
    state(&raw, &conf).map_err(bad)
}
