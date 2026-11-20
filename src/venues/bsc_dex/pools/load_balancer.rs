//! Файл: venues/bsc_dex/pools/load_balancer.rs
//! Что делает: решение 289 — чтение состояния пула Balancer V2 на блоке одной пачкой (Multicall3 / JSON-RPC):
//! Vault.getPoolTokens(id) (токены и полные балансы), getSwapFeePercentage, getPausedState пула и хранилища (ревью 289:
//! Vault.swap — whenNotPaused) и параметр кривой — getNormalizedWeights (весовой) или getAmplificationParameter
//! (стейблсвап, A на блоке с рампой по времени). Неизменяемое
//! у пула — id, вид кривой (какая фабрика его создала: dex.factory — весовой, dex.balancer_stable_factory — стейблсвап;
//! isPoolFromFactory) и decimals токенов (множители точности) — читается один раз на пул и держится в памяти до
//! перезапуска. Собирает PoolState::VaultPool. Балансы меняет хранилище своими событиями — вид перечитывается каждым
//! блоком (per_block.rs).
//! Кто вызывает: pools/load.rs (ветка Proto::BalancerV2), pools/per_block_balancer.rs (calls / decode — все пулы вида за
//! блок одной пачкой).
//! От чего зависит: ctx.rs, node/call.rs, abi_balancer.rs, load_balancer_fixed.rs (неизменяемое у пула).
//! Решение 473: пул вида dex.balancer_pool (composable stable, meta-stable, stable phantom, LBP, investment, managed,
//! linear) — пачка и разбор load_balancer_more*.rs (calls / decode — ветка Fixed.more).
//! Настройки: dex.vault, dex.factory, dex.balancer_stable_factory, dex.balancer_math (решение 450), dex.balancer_pool
//! (решение 473) [DEX]; dex.multicall3 [bsc_dex].
//!
//! Источник — шапка abi_balancer.rs; множитель 10^(18 − decimals) — BasePool.sol (весовой) L474-L481, у стейбла
//! L510-L517 (см. math/balancer_math.rs); точность A — _AMP_PRECISION = 1e3 (StableMath.sol L31, отдаёт
//! getAmplificationParameter, StablePool.sol L615-L626).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};

use crate::core::types::{PoolState, VaultCurve, VaultPoolData};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_balancer::{IBalancerPool as P, IBalancerVault as V};
use super::families::Family;
use super::load_balancer_fixed::{Fixed, fixed};

/// Вызовов на пул в пачке чтения (calls): getPoolTokens, getSwapFeePercentage, getPausedState пула, параметр кривой,
/// getPausedState хранилища.
pub const PER_POOL: usize = 5;

/// Состояние пула на блоке. Принимает: окружение, вид DEX, адрес пула, пару (token0, token1), блок-тег. Ошибки:
/// NotConfigured (dex.vault, dex.factory), узел, ответ не по ABI, состояние несогласовано (BadResponse).
pub async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<PoolState> {
    let (fx, got) = read(ctx, f, pool, tag).await?;
    decode(&fx, &got, pair)
        .map_err(|w| ctx.err(ErrorCode::BadResponse, format!("Balancer {pool}: {w}")))
}

/// Что делает: сетевое чтение пула без пары (первая половина load; решение 295 — одно чтение на все пары пула,
/// pools/load_site.rs). Принимает: окружение, вид DEX, пул, блок-тег. Возвращает: неизменяемое пула и ответы calls.
/// Ошибки: NotConfigured (dex.vault, dex.factory), узел, ответ не по ABI.
pub async fn read(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tag: &str,
) -> GatewayResult<(Fixed, Vec<Result<Bytes, String>>)> {
    let vault = ctx.dex_addr("dex.vault", f.id)?;
    let fx = fixed(ctx, f, vault, pool, tag).await?;
    let got = call::try_many(ctx, &calls(vault, pool, &fx), tag).await?;
    Ok((fx, got))
}

/// Вызовы пачки чтения пула (PER_POOL штук, порядок — как разбирает decode). Пауза хранилища — getPausedState Vault
/// (Vault.swap — whenNotPaused, Swaps.sol L66; TemporarilyPausable.sol L70-L83). Ошибок нет.
pub fn calls(vault: Address, pool: Address, fx: &Fixed) -> Vec<(Address, Bytes)> {
    // Решение 473: вид dex.balancer_pool — своя пачка (load_balancer_more.rs; первые PER_POOL вызовов — те же).
    if let Some(m) = &fx.more {
        return super::load_balancer_more::calls(vault, pool, fx.id, m);
    }
    let curve: Bytes = if fx.weighted {
        P::getNormalizedWeightsCall {}.abi_encode().into()
    } else {
        P::getAmplificationParameterCall {}.abi_encode().into()
    };
    vec![
        (
            vault,
            V::getPoolTokensCall { poolId: fx.id }.abi_encode().into(),
        ),
        (pool, P::getSwapFeePercentageCall {}.abi_encode().into()),
        (pool, P::getPausedStateCall {}.abi_encode().into()),
        (pool, curve),
        (vault, P::getPausedStateCall {}.abi_encode().into()),
    ]
}

/// Разбор ответов пачки (без сети). Принимает: неизменяемое пула, ответы calls по порядку, пару. Возвращает: VaultPool;
/// пул или хранилище на паузе — paused = true (котировки нет). Ошибки (текст): откат вызова, ответ не по ABI, точность A
/// ≠ 1000, токены пула изменились, несогласованное состояние (state).
pub fn decode(
    fx: &Fixed,
    got: &[Result<Bytes, String>],
    pair: (Address, Address),
) -> Result<PoolState, String> {
    // Решение 473: вид dex.balancer_pool — свой разбор (load_balancer_more_curve.rs).
    if let Some(m) = &fx.more {
        return super::load_balancer_more_curve::decode(fx, m, got, pair);
    }
    let ok = |i: usize| match got.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!("вызов {i} откатился: {r}")),
        None => Err(format!("нет ответа на вызов {i}")),
    };
    let dec = |e: alloy::sol_types::Error| format!("ответ не по ABI: {e}");
    let pt = V::getPoolTokensCall::abi_decode_returns(ok(0)?).map_err(dec)?;
    let fee = P::getSwapFeePercentageCall::abi_decode_returns(ok(1)?).map_err(dec)?;
    let paused = P::getPausedStateCall::abi_decode_returns(ok(2)?)
        .map_err(dec)?
        .paused;
    let vault_paused = P::getPausedStateCall::abi_decode_returns(ok(4)?)
        .map_err(dec)?
        .paused;
    // Решение 450: fx.v2 (dex.balancer_math = v2) — кривая библиотек v2 (math/balancer_math.rs).
    let curve = if fx.weighted {
        let weights = P::getNormalizedWeightsCall::abi_decode_returns(ok(3)?).map_err(dec)?;
        if fx.v2 {
            VaultCurve::WeightedV2 { weights }
        } else {
            VaultCurve::Weighted { weights }
        }
    } else {
        let a = P::getAmplificationParameterCall::abi_decode_returns(ok(3)?).map_err(dec)?;
        if a.precision != U256::from(1_000u32) {
            return Err(format!(
                "точность A {} ≠ 1000 (_AMP_PRECISION StableMath.sol L31)",
                a.precision
            ));
        }
        if fx.v2 {
            VaultCurve::StableV2 { amp: a.value }
        } else {
            VaultCurve::Stable { amp: a.value }
        }
    };
    let scaling: Option<Vec<U256>> = pt
        .tokens
        .iter()
        .map(|t| fx.scaling.get(t).copied())
        .collect();
    let scaling = scaling.ok_or("токены пула в хранилище изменились — перезапуск чтения")?;
    state(
        fx.id,
        &pt.tokens,
        pt.balances,
        scaling,
        fee,
        paused || vault_paused,
        curve,
        pair,
    )
}

/// Сборка состояния (без сети; её же зовут тесты на записанных ответах). Принимает: id, токены и балансы хранилища,
/// множители точности, комиссию над 1e18, паузу, кривую, пару. Возвращает: VaultPool. Ошибки (текст): длины не совпали,
/// монеты пары нет среди токенов пула, комиссия ≥ 100 %.
#[allow(clippy::too_many_arguments)]
pub fn state(
    id: B256,
    tokens: &[Address],
    balances: Vec<U256>,
    scaling: Vec<U256>,
    fee: U256,
    paused: bool,
    curve: VaultCurve,
    pair: (Address, Address),
) -> Result<PoolState, String> {
    let n = tokens.len();
    let weights_ok = match &curve {
        VaultCurve::Weighted { weights } | VaultCurve::WeightedV2 { weights } => weights.len() == n,
        // Решение 473: кривые других фабрик — длины сверяет их разбор (load_balancer_more_curve.rs).
        VaultCurve::Stable { .. } | VaultCurve::StableV2 { .. } | VaultCurve::V2(_) => true,
        // Решение 452: кривая пула Balancer V3 — у пула V2 не бывает.
        VaultCurve::V3 => false,
    };
    if n < 2 || balances.len() != n || scaling.len() != n || !weights_ok {
        return Err(format!(
            "длины токенов / балансов / множителей / весов не совпали (токенов {n})"
        ));
    }
    if fee >= U256::from(10u8).pow(U256::from(18u8)) {
        return Err(format!("комиссия {fee} ≥ 1e18"));
    }
    let index = |t: Address| {
        tokens
            .iter()
            .position(|x| *x == t)
            .map(|p| p as u8)
            .ok_or(format!("монеты {t} нет среди токенов пула"))
    };
    let (i0, i1) = (index(pair.0)?, index(pair.1)?);
    Ok(PoolState::VaultPool(Box::new(VaultPoolData {
        pool_id: id,
        balances,
        scaling,
        fee,
        curve,
        paused,
        i0,
        i1,
        // Решение 441: часть хранилища Balancer V3 — у пула V2 её нет.
        v3: None,
    })))
}
