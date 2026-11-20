//! Файл: venues/bsc_dex/pools/load_crypto_v1.rs
//! Что делает: чтение состояния crypto-пула Curve первого поколения (решение 453: фабрика crypto v1 — 2 монеты,
//! tricrypto 2021 — 3) на блоке одним try_many (Multicall3): A(), gamma(), D(), mid_fee(), out_fee(), fee_gamma(),
//! future_A_gamma_time(), coins(k), balances(k), price_scale (2 монеты — price_scale(), 3 — price_scale(0..1)),
//! у tricrypto — is_killed(), и время блока (как load_crypto.rs). Множители точности: у пулов v1 геттера нет —
//! 10^(18 − decimals) монеты (tokens.rs): так их задаёт фабрика v1 (Factory.vy deploy_pool L194-L199: precisions =
//! (18 − decimals[0]) + (18 − decimals[1]) << 8, пул — `10 ** …`, CurveCryptoSwap2ETH.vy `_get_precisions` L181-L185) и
//! такие же числа стоят в развёрнутом tricrypto («2021-09-13. Arbitrum/CryptoSwap.vy» PRECISIONS L188-L192: 10^12,
//! 10^10, 1 у USDT, WBTC, WETH). Рампа у v1 (`if t > 0`, CurveCryptoSwap2ETH.vy L676, CryptoSwap.vy L637): обмен
//! пересчитывает D всегда, пока future_A_gamma_time > 0 (после конца рампы пул ставит 1) — ramp; A и gamma меняются
//! по времени блока, пока время блока < future_A_gamma_time — moving (перечитывание каждым блоком, per_block.rs).
//! Кто вызывает: pools/load.rs, pools/load_site.rs (пары tricrypto).
//! От чего зависит: ctx.rs, node/call.rs, node/multicall.rs, tokens.rs, abi_crypto.rs, abi_crypto_v1.rs, load_lb.rs
//! (время блока Multicall3), load_smardex.rs (время блока без Multicall3), families.rs.
//! Настройки: dex.multicall3 [bsc_dex] (как load_crypto.rs).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{CryptoData, CryptoMath, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::ten;
use super::super::node::{call, multicall};
use super::super::tokens::Tokens;
use super::abi_crypto::{ICryptoPool as P, ITricryptoPool as P3};
use super::abi_crypto_v1::ICryptoPoolV1 as V1;
use super::families::{Family, Proto};
use super::load_lb::{decode_time, time_call};
use super::load_smardex;

/// Вызовов до монет: A, gamma, D, mid_fee, out_fee, fee_gamma, future_A_gamma_time.
const HEAD: usize = 7;

/// Прочитанное: пул, число монет, ответы, время блока, decimals монет (по порядку монет пула).
pub struct Raw {
    pub pool: Address,
    pub n: usize,
    pub got: Vec<Result<Bytes, String>>,
    pub now: u64,
    pub decimals: Vec<u8>,
}

/// Число монет вида: фабрика v1 — 2, tricrypto 2021 — 3. None — не вид v1. Ошибок нет.
pub fn coins_of(f: &Family) -> Option<usize> {
    match f.proto {
        Proto::CurveCryptoV1 => Some(2),
        Proto::CurveTricryptoV1 => Some(3),
        _ => None,
    }
}

/// Вызовы чтения по порядку: HEAD, coins(k)/balances(k), price_scale, у трёх монет — is_killed(). Ошибок нет.
pub fn calls(pool: Address, n: usize) -> Vec<(Address, Bytes)> {
    let mut data: Vec<Bytes> = vec![
        P::ACall {}.abi_encode().into(),
        P::gammaCall {}.abi_encode().into(),
        P::DCall {}.abi_encode().into(),
        P::mid_feeCall {}.abi_encode().into(),
        P::out_feeCall {}.abi_encode().into(),
        P::fee_gammaCall {}.abi_encode().into(),
        P::future_A_gamma_timeCall {}.abi_encode().into(),
    ];
    for k in 0..n {
        let i = U256::from(k);
        data.push(P::coinsCall { i }.abi_encode().into());
        data.push(P::balancesCall { i }.abi_encode().into());
    }
    if n == 2 {
        data.push(V1::price_scaleCall {}.abi_encode().into());
    } else {
        for k in 0..n - 1 {
            data.push(P3::price_scaleCall { k: U256::from(k) }.abi_encode().into());
        }
        data.push(V1::is_killedCall {}.abi_encode().into());
    }
    data.into_iter().map(|d| (pool, d)).collect()
}

/// Что делает: сетевое чтение пула. Принимает: окружение, токены (decimals), вид, пул, блок. Возвращает: Raw.
/// Ошибки: вид не v1, узел, ответов не столько, сколько вызовов, время блока, decimals монеты.
pub async fn read(
    ctx: &Ctx,
    tokens: &Tokens,
    f: &Family,
    pool: Address,
    block: u64,
) -> GatewayResult<Raw> {
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("Crypto v1 {pool}: {what}"));
    let Some(n) = coins_of(f) else {
        return Err(bad(format!("{}: вид не crypto v1", f.title)));
    };
    let mut all = calls(pool, n);
    let mc = multicall::address(ctx);
    if let Some(m) = mc {
        all.push(time_call(m));
    }
    let mut got = call::try_many(ctx, &all, &call::block_tag(block)).await?;
    if got.len() != all.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), all.len())));
    }
    let now = match mc {
        Some(_) => {
            let t = got.pop().unwrap_or_else(|| Err("нет ответа".into()));
            let t = t.map_err(|w| bad(format!("время блока откатилось: {w}")))?;
            decode_time(&t).map_err(&bad)?
        }
        None => load_smardex::block_secs(ctx, block).await?,
    };
    let mut decimals = Vec::with_capacity(n);
    for k in 0..n {
        let Some(Ok(b)) = got.get(HEAD + 2 * k) else {
            return Err(bad(format!("coins({k}) откатился")));
        };
        let coin = call::decode::<P::coinsCall>(b)?;
        decimals.push(tokens.decimals(ctx, coin).await?);
    }
    Ok(Raw {
        pool,
        n,
        got,
        now,
        decimals,
    })
}

/// Состояние пары на блоке. Принимает: окружение, токены, вид, пул, пару, блок. Ошибки: как read и state.
pub async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    block: u64,
) -> GatewayResult<PoolState> {
    state(ctx, &read(ctx, tokens, f, pool, block).await?, pair)
}

/// Что делает: состояние пары из прочитанного (без сети). Принимает: окружение (текст ошибки), Raw, пару.
/// Возвращает: PoolState::Crypto. Ошибки: откат вызова, ответ не по ABI, decimals > 18, нет монеты пары в пуле.
pub fn state(ctx: &Ctx, raw: &Raw, pair: (Address, Address)) -> GatewayResult<PoolState> {
    let pool = raw.pool;
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("Crypto v1 {pool}: {what}"));
    let (coins, mut data) = decode(raw).map_err(&bad)?;
    let index = |t: Address| {
        coins
            .iter()
            .position(|c| *c == t)
            .map(|p| p as u8)
            .ok_or_else(|| bad(format!("монеты {t} нет среди монет пула")))
    };
    data.i0 = index(pair.0)?;
    data.i1 = index(pair.1)?;
    Ok(PoolState::Crypto(Box::new(data)))
}

/// Что делает: разбор ответов (без сети). Принимает: Raw. Возвращает: (монеты, CryptoData с индексами пары 0) или текст
/// причины (откат вызова, ответ не по ABI, decimals > 18).
pub fn decode(raw: &Raw) -> Result<(Vec<Address>, CryptoData), String> {
    let got = &raw.got;
    let ok = |i: usize| match got.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!("вызов {i} откатился: {r}")),
        None => Err(format!("нет ответа на вызов {i}")),
    };
    let w = |i: usize| -> Result<U256, String> {
        let v = ok(i)?.get(..32).ok_or(format!("ответ {i} короче 32 байт"))?;
        Ok(U256::from_be_slice(v))
    };
    let mut coins = Vec::with_capacity(raw.n);
    let mut balances = Vec::with_capacity(raw.n);
    let mut precisions = Vec::with_capacity(raw.n);
    for k in 0..raw.n {
        let c = P::coinsCall::abi_decode_returns(ok(HEAD + 2 * k)?).map_err(|e| e.to_string())?;
        coins.push(c);
        balances.push(w(HEAD + 2 * k + 1)?);
        let d = *raw.decimals.get(k).ok_or("нет decimals монеты")?;
        let p = 18u64.checked_sub(d.into()).ok_or(format!("decimals {d} > 18"))?;
        precisions.push(ten::pow(p));
    }
    let tail = HEAD + 2 * raw.n;
    let price_scale = (0..raw.n - 1)
        .map(|k| w(tail + k))
        .collect::<Result<Vec<_>, _>>()?;
    // is_killed() — только у tricrypto 2021 (exchange: assert not self.is_killed, CryptoSwap.vy L594).
    let halted = raw.n == 3 && !w(tail + raw.n - 1)?.is_zero();
    let t = w(6)?;
    let data = CryptoData {
        balances,
        precisions,
        price_scale,
        d: w(2)?,
        a: w(0)?,
        gamma: w(1)?,
        mid_fee: w(3)?,
        out_fee: w(4)?,
        fee_gamma: w(5)?,
        math: if raw.n == 2 { CryptoMath::TwoV1 } else { CryptoMath::TriV1 },
        ramp: !t.is_zero(),
        moving: t > U256::from(raw.now),
        halted,
        i0: 0,
        i1: 0,
    };
    Ok((coins, data))
}
