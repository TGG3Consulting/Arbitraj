//! Файл: venues/bsc_dex/pools/load_camelot.rs
//! Что делает: решение 443 (T433-F4) — чтение состояния пары Camelot V2 на блоке одним try_many (Multicall3):
//! getReserves() (резервы uint112 и комиссии входа token0FeePercent / token1FeePercent), stableSwap(),
//! precisionMultiplier0(), precisionMultiplier1(). Собирает PoolState::Solidly: резервы, флаг stable, множители точности
//! (10^decimals — прямо в decimals0/1, как dec0/dec1 у Solidly), комиссия входа token0 — `fee`, входа token1 — `fee1`
//! (правило «числитель над FEE_DENOMINATOR = 100000», Pair.sol L379, L391), ограничений обмена нет.
//! Всё, что меняет состояние, пара шлёт событием (Sync, FeePercentUpdated, SetStableSwap — events_camelot.rs);
//! множители точности задаются один раз в initialize (Pair.sol L80-L90) — каждым блоком не перечитывается.
//! Кто вызывает: pools/load.rs (ветка Proto::CamelotV2).
//! От чего зависит: ctx.rs, node/call.rs, abi_camelot.rs, math/camelot_v2.rs (FEE_DENOMINATOR, MAX_FEE_PERCENT).
//! Настройки: dex.multicall3 [площадка DEX] (не задан — пачкой JSON-RPC, call::try_many).
//!
//! Источник — verified-исходник CamelotPair (abi_camelot.rs, «Pair.sol»): getReserves L46-L51, precisionMultiplier0/1
//! L30-L31 и L86-L87, stableSwap L35, FEE_DENOMINATOR L22, MAX_FEE_PERCENT L23 и предел setFeePercent L99.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{FeeRule, PoolState, SolidlyFee, SolidlyGate};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::camelot_v2::{FEE_DENOMINATOR, MAX_FEE_PERCENT};
use super::super::node::call;
use super::abi_camelot::ICamelotPair as P;

/// Прочитанное у пары: резервы, комиссии входа token0 / token1, stableSwap, множители точности.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Read {
    pub reserve0: U256,
    pub reserve1: U256,
    pub fee0: u16,
    pub fee1: u16,
    pub stable: bool,
    pub pm0: U256,
    pub pm1: U256,
}

/// Состояние пары на блоке. Принимает: окружение, адрес пары, блок-тег. Ошибки: узел, вызов откатился, ответ не по
/// ABI, состояние не пары Camelot (state).
pub async fn load(ctx: &Ctx, pool: Address, tag: &str) -> GatewayResult<PoolState> {
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("Camelot V2 {pool}: {what}"));
    let calls: Vec<(Address, Bytes)> = [
        P::getReservesCall {}.abi_encode(),
        P::stableSwapCall {}.abi_encode(),
        P::precisionMultiplier0Call {}.abi_encode(),
        P::precisionMultiplier1Call {}.abi_encode(),
    ]
    .into_iter()
    .map(|d| (pool, Bytes::from(d)))
    .collect();
    let got = call::try_many(ctx, &calls, tag).await?;
    if got.len() != calls.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), calls.len())));
    }
    let ok = |i: usize| {
        got[i]
            .as_ref()
            .map_err(|r| bad(format!("вызов {i} откатился: {r}")))
    };
    let r = call::decode::<P::getReservesCall>(ok(0)?)?;
    let read = Read {
        reserve0: U256::from(r.reserve0),
        reserve1: U256::from(r.reserve1),
        fee0: r.token0FeePercent,
        fee1: r.token1FeePercent,
        stable: call::decode::<P::stableSwapCall>(ok(1)?)?,
        pm0: call::decode::<P::precisionMultiplier0Call>(ok(2)?)?,
        pm1: call::decode::<P::precisionMultiplier1Call>(ok(3)?)?,
    };
    state(&read).map_err(|w| bad(w.to_string()))
}

/// Сборка состояния из прочитанного (без сети; её же зовут тесты на записанных ответах). Принимает: прочитанное.
/// Возвращает: PoolState::Solidly с fee1. Ошибки: множитель точности 0 (пара не инициализирована — initialize ставит
/// 10^decimals, L86-L87); комиссия больше MAX_FEE_PERCENT или 0 (пара так не позволяет — L99, L100: это не пара
/// Camelot V2).
pub fn state(r: &Read) -> Result<PoolState, &'static str> {
    if r.pm0.is_zero() || r.pm1.is_zero() {
        return Err("множитель точности 0 — пара не инициализирована");
    }
    let max = u32::from(r.fee0.max(r.fee1));
    if max > MAX_FEE_PERCENT {
        return Err("комиссия больше MAX_FEE_PERCENT — не пара Camelot V2");
    }
    // L27-L28: по умолчанию 300; setFeePercent требует > 0 (L100) — нулевой комиссии у пары не бывает.
    if r.fee0 == 0 || r.fee1 == 0 {
        return Err("комиссия 0 — у пары Camelot V2 так не бывает (L27-L28, L100)");
    }
    let fee = |value: u16| SolidlyFee {
        value: u64::from(value),
        rule: FeeRule::Numerator {
            denominator: u64::from(FEE_DENOMINATOR),
        },
    };
    Ok(PoolState::Solidly {
        reserve0: r.reserve0,
        reserve1: r.reserve1,
        stable: r.stable,
        decimals0: r.pm0,
        decimals1: r.pm1,
        fee: fee(r.fee0),
        gate: SolidlyGate::default(),
        fee1: Some(fee(r.fee1)),
    })
}
