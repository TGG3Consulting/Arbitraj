//! Файл: venues/bsc_dex/pools/load_balancer_v3.rs
//! Что делает: решение 441 (T433-F2) — чтение состояния пула Balancer V3 на блоке одной пачкой (Multicall3 / JSON-RPC):
//! у хранилища Vault V3 — getPoolTokenInfo (токены и сырые балансы), getCurrentLiveBalances (живые балансы 18 знаков,
//! как их передаёт пулу Vault.swap), getPoolTokenRates (множители точности и курсы), getPoolConfig (комиссия,
//! инициализирован ли пул), isPoolPaused, isVaultPaused (Vault.swap — _ensureUnpaused); у стейблсвапа ещё
//! getAmplificationParameter (A на блоке с рампой по времени). Неизменяемое — load_balancer_v3_fixed.rs. Собирает
//! PoolState::VaultPool с частью V3. Балансы и курсы меняются без событий пула — вид перечитывается каждым блоком.
//! Кто вызывает: pools/load.rs и pools/load_site.rs (ветка Proto::BalancerV3).
//! От чего зависит: ctx.rs, node/call.rs, abi_balancer_v3.rs, load_balancer_v3_fixed.rs.
//! Настройки: dex.vault, dex.bv3_* [DEX вида balancer_v3]; dex.multicall3 [площадка DEX].
//!
//! Источник — шапка abi_balancer_v3.rs; Vault.sol (20241204-v3-vault) L197-L222 (swap: _ensureUnpaused,
//! withInitializedPool, _loadPoolDataUpdatingBalancesAndYieldFees(ROUND_DOWN) = getCurrentLiveBalances —
//! VaultExtension.sol L551-L555), PoolConfigLib.sol L241-L261 (множитель 10^(18 − decimals)); точность A — AMP_PRECISION
//! = 1e3 (StableMath.sol L38, отдаёт getAmplificationParameter).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::pool::PoolState;
use crate::core::types::pool_vault::{VaultCurve, VaultPoolData, VaultV3, VaultV3Rule};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_balancer_v3::{IBalancerV3Pool as P, IBalancerV3Vault as V};
use super::balancer_v3_factories::Release;
use super::families::Family;
use super::load_balancer_v3_fixed::{Fixed, fixed};

/// Состояние пула на блоке. Принимает: окружение, вид DEX, пул, пару, блок-тег. Ошибки: NotConfigured (dex.vault,
/// dex.bv3_*), узел, ответ не по ABI, пул не принят (крючок обмена, не из фабрик), несогласованное состояние.
pub async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<PoolState> {
    let (fx, got) = read(ctx, f, pool, tag).await?;
    decode(pool, &fx, &got, pair)
        .map_err(|w| ctx.err(ErrorCode::BadResponse, format!("Balancer V3 {pool}: {w}")))
}

/// Сетевое чтение пула без пары (одно чтение на все пары пула, решение 295). Принимает: окружение, вид, пул, блок-тег.
/// Возвращает: неизменяемое и ответы calls. Ошибки: как load.
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

/// Вызовы пачки чтения (порядок — как разбирает decode; у стейблсвапа седьмой — A пула). Ошибок нет.
pub fn calls(vault: Address, pool: Address, fx: &Fixed) -> Vec<(Address, Bytes)> {
    let mut l: Vec<(Address, Bytes)> = vec![
        (vault, V::getPoolTokenInfoCall { pool }.abi_encode().into()),
        (vault, V::getCurrentLiveBalancesCall { pool }.abi_encode().into()),
        (vault, V::getPoolTokenRatesCall { pool }.abi_encode().into()),
        (vault, V::getPoolConfigCall { pool }.abi_encode().into()),
        (vault, V::isPoolPausedCall { pool }.abi_encode().into()),
        (vault, V::isVaultPausedCall {}.abi_encode().into()),
    ];
    if !fx.release.weighted() {
        l.push((pool, P::getAmplificationParameterCall {}.abi_encode().into()));
    }
    l
}

/// Разбор ответов пачки (без сети; его же зовут тесты на записанных ответах). Принимает: пул, неизменяемое, ответы
/// calls, пару. Возвращает: VaultPool с частью V3; пауза пула / хранилища или пул не инициализирован — paused = true.
/// Ошибки (текст): откат вызова, не по ABI, точность A ≠ 1000, токены изменились, длины не совпали, монеты пары нет.
pub fn decode(
    pool: Address,
    fx: &Fixed,
    got: &[Result<Bytes, String>],
    pair: (Address, Address),
) -> Result<PoolState, String> {
    let ok = |i: usize| match got.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!("вызов {i} откатился: {r}")),
        None => Err(format!("нет ответа на вызов {i}")),
    };
    let dec = |e: alloy::sol_types::Error| format!("ответ не по ABI: {e}");
    let info = V::getPoolTokenInfoCall::abi_decode_returns(ok(0)?).map_err(dec)?;
    if info.tokens != fx.tokens {
        return Err("токены пула в хранилище изменились — перезапуск чтения".into());
    }
    let live = V::getCurrentLiveBalancesCall::abi_decode_returns(ok(1)?).map_err(dec)?;
    let rates = V::getPoolTokenRatesCall::abi_decode_returns(ok(2)?).map_err(dec)?;
    let cfg = V::getPoolConfigCall::abi_decode_returns(ok(3)?).map_err(dec)?;
    let pool_paused = V::isPoolPausedCall::abi_decode_returns(ok(4)?).map_err(dec)?;
    let vault_paused = V::isVaultPausedCall::abi_decode_returns(ok(5)?).map_err(dec)?;
    let (curve, rule) = match fx.release {
        Release::Weighted | Release::WeightedMin => {
            let rule = if fx.release == Release::WeightedMin {
                VaultV3Rule::WeightedMin {
                    min: fx.min_balances.clone(),
                }
            } else {
                VaultV3Rule::Weighted
            };
            let w = fx.weights.clone();
            (VaultCurve::Weighted { weights: w }, rule)
        }
        Release::Stable | Release::StableGuard => {
            let a = P::getAmplificationParameterCall::abi_decode_returns(ok(6)?).map_err(dec)?;
            if a.precision != U256::from(1_000u32) {
                return Err(format!("точность A {} ≠ 1000 (StableMath.sol L38)", a.precision));
            }
            let rule = if fx.release == Release::StableGuard {
                VaultV3Rule::StableGuard
            } else {
                VaultV3Rule::Stable
            };
            (VaultCurve::Stable { amp: a.value }, rule)
        }
    };
    let v3 = VaultV3 {
        live,
        rates: rates.tokenRates,
        min_trade: fx.min_trade,
        rule,
    };
    let paused = pool_paused || vault_paused || !cfg.isPoolInitialized;
    let parts = Parts {
        pool,
        tokens: &info.tokens,
        balances: info.balancesRaw,
        scaling: rates.decimalScalingFactors,
        fee: cfg.staticSwapFeePercentage,
        paused,
        curve,
    };
    state(parts, v3, pair)
}

/// Части состояния пула без части V3 (сборка — state).
pub struct Parts<'a> {
    pub pool: Address,
    pub tokens: &'a [Address],
    pub balances: Vec<U256>,
    pub scaling: Vec<U256>,
    pub fee: U256,
    pub paused: bool,
    pub curve: VaultCurve,
}

/// Сборка состояния (без сети). Принимает: части, часть V3, пару. Возвращает: VaultPool. Ошибки (текст): длины токенов
/// / балансов / множителей / курсов / весов / нижних балансов не совпали, монеты пары нет среди токенов, комиссия ≥ 100 %.
pub fn state(p: Parts, v3: VaultV3, pair: (Address, Address)) -> Result<PoolState, String> {
    let n = p.tokens.len();
    let weights_ok = match &p.curve {
        VaultCurve::Weighted { weights } => weights.len() == n,
        VaultCurve::Stable { .. } => true,
    };
    let min_ok = match &v3.rule {
        VaultV3Rule::WeightedMin { min } => min.len() == n,
        _ => true,
    };
    let lens = [p.balances.len(), p.scaling.len(), v3.live.len(), v3.rates.len()];
    if n < 2 || lens.iter().any(|l| *l != n) || !weights_ok || !min_ok {
        return Err(format!(
            "длины токенов / балансов / множителей / курсов / весов не совпали (токенов {n})"
        ));
    }
    if p.fee >= U256::from(10u8).pow(U256::from(18u8)) {
        return Err(format!("комиссия {} ≥ 1e18", p.fee));
    }
    let index = |t: Address| {
        p.tokens
            .iter()
            .position(|x| *x == t)
            .map(|i| i as u8)
            .ok_or(format!("монеты {t} нет среди токенов пула"))
    };
    let (i0, i1) = (index(pair.0)?, index(pair.1)?);
    Ok(PoolState::VaultPool(Box::new(VaultPoolData {
        // Id пула V3 — его адрес (topic1 события Swap хранилища — адрес пула, IVaultEvents.sol L53-L61).
        pool_id: p.pool.into_word(),
        balances: p.balances,
        scaling: p.scaling,
        fee: p.fee,
        curve: p.curve,
        paused: p.paused,
        i0,
        i1,
        v3: Some(Box::new(v3)),
    })))
}
