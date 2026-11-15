//! Файл: venues/bsc_dex/pools/load_balancer.rs
//! Что делает: решение 289 — чтение состояния пула Balancer V2 на блоке одной пачкой (Multicall3 / JSON-RPC):
//! Vault.getPoolTokens(id) (токены и полные балансы), getSwapFeePercentage, getPausedState и параметр кривой —
//! getNormalizedWeights (весовой) или getAmplificationParameter (стейблсвап, A на блоке с рампой по времени). Неизменяемое
//! у пула — id, вид кривой (какая фабрика его создала: dex.factory — весовой, dex.balancer_stable_factory — стейблсвап;
//! isPoolFromFactory) и decimals токенов (множители точности) — читается один раз на пул и держится в памяти до
//! перезапуска. Собирает PoolState::VaultPool. Балансы меняет хранилище своими событиями — вид перечитывается каждым
//! блоком (per_block.rs).
//! Кто вызывает: pools/load.rs (ветка Proto::BalancerV2).
//! От чего зависит: ctx.rs, node/call.rs, abi_balancer.rs, load_balancer_fixed.rs (неизменяемое у пула).
//! Настройки: dex.vault, dex.factory, dex.balancer_stable_factory [DEX]; dex.multicall3 [bsc_dex].
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
use super::load_balancer_fixed::fixed;

/// Состояние пула на блоке. Принимает: окружение, вид DEX, адрес пула, пару (token0, token1), блок-тег. Ошибки:
/// NotConfigured (dex.vault, dex.factory), узел, ответ не по ABI, состояние несогласовано (BadResponse).
pub async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<PoolState> {
    let vault = ctx.dex_addr("dex.vault", f.id)?;
    let fx = fixed(ctx, f, vault, pool, tag).await?;
    let curve: Bytes = if fx.weighted {
        P::getNormalizedWeightsCall {}.abi_encode().into()
    } else {
        P::getAmplificationParameterCall {}.abi_encode().into()
    };
    let calls: Vec<(Address, Bytes)> = vec![
        (
            vault,
            V::getPoolTokensCall { poolId: fx.id }.abi_encode().into(),
        ),
        (pool, P::getSwapFeePercentageCall {}.abi_encode().into()),
        (pool, P::getPausedStateCall {}.abi_encode().into()),
        (pool, curve),
    ];
    let bad = |w: String| ctx.err(ErrorCode::BadResponse, format!("Balancer {pool}: {w}"));
    let got = call::try_many(ctx, &calls, tag).await?;
    let ok = |i: usize| {
        got.get(i)
            .and_then(|r| r.as_ref().ok())
            .ok_or_else(|| bad(format!("вызов {i} откатился")))
    };
    let pt = call::decode::<V::getPoolTokensCall>(ok(0)?)?;
    let fee = call::decode::<P::getSwapFeePercentageCall>(ok(1)?)?;
    let paused = call::decode::<P::getPausedStateCall>(ok(2)?)?.paused;
    let curve = if fx.weighted {
        VaultCurve::Weighted {
            weights: call::decode::<P::getNormalizedWeightsCall>(ok(3)?)?,
        }
    } else {
        let a = call::decode::<P::getAmplificationParameterCall>(ok(3)?)?;
        if a.precision != U256::from(1_000u32) {
            return Err(bad(format!(
                "точность A {} ≠ 1000 (_AMP_PRECISION StableMath.sol L31)",
                a.precision
            )));
        }
        VaultCurve::Stable { amp: a.value }
    };
    let scaling: Option<Vec<U256>> = pt
        .tokens
        .iter()
        .map(|t| fx.scaling.get(t).copied())
        .collect();
    let scaling = scaling
        .ok_or_else(|| bad("токены пула в хранилище изменились — перезапуск чтения".into()))?;
    state(
        fx.id,
        &pt.tokens,
        pt.balances,
        scaling,
        fee,
        paused,
        curve,
        pair,
    )
    .map_err(bad)
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
        VaultCurve::Weighted { weights } => weights.len() == n,
        VaultCurve::Stable { .. } => true,
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
    })))
}
