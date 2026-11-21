//! Файл: venues/bsc_dex/pools/load_curve_ng.rs
//! Что делает: чтение состояния пула Curve StableSwap-NG (решение 275) на блоке одним try_many (Multicall3): N_COINS,
//! coins(k), balances(k) (без доли админа; у rebasing-монеты — balanceOf пула на этом блоке), stored_rates() (курсы
//! монет: множитель точности; у монеты с оракулом курса и ERC4626 пул сам зовёт внешний контракт на этом блоке),
//! A_precise() (A·100 с рампой по времени блока), fee(), offpeg_fee_multiplier(). Собирает PoolState::StableSwap с
//! шаблоном NG (математика — math/stableswap_ng.rs, та же, что у Infinity Stable: формулы совпадают с
//! CurveStableSwapNG.vy v7.0.0, разбор — docs/notes/curve-ng-2026-11-14.md). Курсы и rebasing-балансы меняются без
//! событий пула — вид перечитывается на каждом блоке (pools/per_block.rs). Откат stored_rates() (оракул курса не
//! отвечает) — обмен откатился бы тоже: пул «остановлен» (halted) до следующего удачного чтения.
//! N_COINS — неизменяемое (public immutable) — читается один раз на пул и держится в памяти до перезапуска.
//! Кто вызывает: pools/load.rs (ветка Proto::CurveNg).
//! От чего зависит: ctx.rs, node/call.rs, abi_curve_ng.rs, math/stableswap_ng.rs (A_PRECISION, MAX_ROUNDS).
//! Настройки: dex.multicall3 [bsc_dex] (не задан — пачкой JSON-RPC, call::try_many).
//!
//! Источник — curvefi/stableswap-ng 3332cd4, contracts/main/CurveStableSwapNG.vy (строки — abi_curve_ng.rs):
//! `_stored_rates` L433-L469, `_balances` L473-L500, `_A` L1187-L1205, `_xp_mem` L1209-L1218.

use std::collections::HashMap;
use std::sync::Mutex;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolState, StableData, StableNg};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::stableswap_ng::{A_PRECISION, MAX_ROUNDS};
use super::super::node::call;
use super::abi_curve_ng::{ICurveNgPool as P, NG_MAX_COINS};

/// N_COINS прочитанных пулов (неизменяемое у пула — CurveStableSwapNG.vy L157, public immutable). Решение 435 (433-и):
/// ключ — (площадка DEX, пул): пул с тем же адресом в другой сети — другой контракт.
static N_COINS: Mutex<Option<HashMap<(&'static str, Address), usize>>> = Mutex::new(None);

/// N_COINS пула: из памяти или с узла (один раз на пул). Принимает: окружение, пул, блок-тег. Ошибки: узел, ответ не
/// по ABI, N_COINS вне 2..=8 (не запоминается).
async fn n_coins(ctx: &Ctx, pool: Address, tag: &str) -> GatewayResult<usize> {
    let known = N_COINS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&(ctx.place.id, pool)).copied());
    if let Some(n) = known {
        return Ok(n);
    }
    let n = call::view(ctx, pool, &P::N_COINSCall {}, tag).await?;
    if n < U256::from(2u8) || n > U256::from(NG_MAX_COINS) {
        let what = format!("Curve NG {pool}: N_COINS = {n} вне 2..={NG_MAX_COINS}");
        return Err(ctx.err(ErrorCode::BadResponse, what));
    }
    let n = n.to::<usize>();
    let mut g = N_COINS.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new)
        .insert((ctx.place.id, pool), n);
    Ok(n)
}

/// Состояние пула на блоке. Принимает: окружение, адрес пула, пару (token0, token1), блок-тег. Ошибки: узел, ответ не
/// по ABI, N_COINS вне 2..=8, нет монеты пары среди монет пула.
pub async fn load(
    ctx: &Ctx,
    pool: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<PoolState> {
    state(ctx, &read(ctx, pool, tag).await?, pair)
}

/// Решение 295: ответы пула, прочитанные ОДИН раз для всех его пар (pools/load_site.rs): пул, N_COINS, ответы.
pub struct Raw {
    pub pool: Address,
    pub n: usize,
    pub got: Vec<Result<Bytes, String>>,
}

/// Что делает: сетевое чтение пула без пары (первая половина load, решение 295): N_COINS и одна пачка вызовов.
/// Принимает: окружение, пул, блок-тег. Возвращает: Raw. Ошибки: узел, N_COINS вне 2..=8, ответов не столько, сколько
/// вызовов.
pub async fn read(ctx: &Ctx, pool: Address, tag: &str) -> GatewayResult<Raw> {
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("Curve NG {pool}: {what}"));
    let n = n_coins(ctx, pool, tag).await?;
    let mut calls: Vec<Bytes> = vec![
        P::A_preciseCall {}.abi_encode().into(),
        P::feeCall {}.abi_encode().into(),
        P::offpeg_fee_multiplierCall {}.abi_encode().into(),
        P::stored_ratesCall {}.abi_encode().into(),
    ];
    for k in 0..n {
        let i = U256::from(k);
        calls.push(P::coinsCall { i }.abi_encode().into());
        calls.push(P::balancesCall { i }.abi_encode().into());
    }
    let calls: Vec<(Address, Bytes)> = calls.into_iter().map(|d| (pool, d)).collect();
    let got = call::try_many(ctx, &calls, tag).await?;
    if got.len() != calls.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), calls.len())));
    }
    Ok(Raw { pool, n, got })
}

/// Что делает: состояние пары из прочитанного (вторая половина load, без сети; решение 295 — одно чтение на все
/// пары). Принимает: окружение (текст ошибки), Raw, пару (token0, token1). Ошибки: откат вызова, ответ не по ABI, нет
/// монеты пары среди монет пула.
pub fn state(ctx: &Ctx, raw: &Raw, pair: (Address, Address)) -> GatewayResult<PoolState> {
    let (pool, n, got) = (raw.pool, raw.n, &raw.got);
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("Curve NG {pool}: {what}"));
    let ok = |i: usize| {
        got[i]
            .as_ref()
            .map_err(|r| bad(format!("вызов {i} откатился: {r}")))
    };
    let amp = call::decode::<P::A_preciseCall>(ok(0)?)?;
    let fee = call::decode::<P::feeCall>(ok(1)?)?;
    let offpeg = call::decode::<P::offpeg_fee_multiplierCall>(ok(2)?)?;
    // Откат stored_rates — оракул курса монеты не отвечает: обмен откатился бы (raw_call L447-L452) → пул стоит.
    let rates = match got[3].as_ref() {
        Ok(b) => Some(call::decode::<P::stored_ratesCall>(b)?),
        Err(_) => None,
    };
    let mut coins = Vec::with_capacity(n);
    let mut balances = Vec::with_capacity(n);
    for k in 0..n {
        coins.push(call::decode::<P::coinsCall>(ok(4 + 2 * k)?)?);
        balances.push(call::decode::<P::balancesCall>(ok(5 + 2 * k)?)?);
    }
    let index = |t: Address| {
        coins
            .iter()
            .position(|c| *c == t)
            .map(|p| p as u8)
            .ok_or_else(|| bad(format!("монеты {t} нет среди монет пула")))
    };
    let (i0, i1) = (index(pair.0)?, index(pair.1)?);
    let halted = rates.as_ref().is_none_or(|r| r.len() != n);
    let rates = rates
        .filter(|r| r.len() == n)
        .unwrap_or_else(|| vec![U256::ZERO; n]);
    Ok(PoolState::StableSwap(Box::new(StableData {
        balances,
        rates,
        amp,
        fee,
        amp_precision: U256::from(A_PRECISION),
        max_rounds: MAX_ROUNDS,
        // get_D / get_y пула откатываются при несходимости (raise L1074, L1125).
        strict: true,
        guard: None,
        halted,
        i0,
        i1,
        ng: Some(StableNg {
            offpeg_fee_multiplier: offpeg,
        }),
        // Монеты plain-пула NG — только ERC-20 (нативной метки нет).
        native: None,
        input_fee: false,
        pair_d: false,
        ema: false,
        wombat: None,
    })))
}
