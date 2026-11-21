//! Файл: venues/bsc_dex/pools/load_crypto.rs
//! Что делает: чтение состояния crypto-пула Curve (решение 442: Twocrypto-NG — 2 монеты, Tricrypto-NG — 3) на блоке
//! одним try_many (Multicall3): coins(k), balances(k), precisions(), price_scale (Twocrypto — price_scale(), Tricrypto —
//! price_scale(0), price_scale(1)), D(), A(), gamma(), mid_fee(), out_fee(), fee_gamma(), future_A_gamma_time(), MATH()
//! и время блока (Multicall3.getCurrentBlockTimestamp в той же пачке; без dex.multicall3 — eth_getBlockByNumber).
//! Собирает PoolState::Crypto. Решение 453: версия формул — по MATH() пула: адрес из dex.curve_crypto_math_v2_0_0 —
//! Twocrypto-NG v2.0.0, из dex.curve_crypto_math — v2.1.0 / Tricrypto-NG, иначе отказ. Рампа A/gamma
//! (future_A_gamma_time > время блока; `_exchange` Twocrypto L770-L781, Tricrypto L901-L913) — ramp и moving: обмен
//! пересчитает D через newton_D (math/crypto_swap.rs), A() и gamma() пул отдаёт на время блока чтения, пул
//! перечитывается каждым блоком, пока рампа идёт (per_block.rs). Решение 455: Tricrypto-NG с монетой dex.native_wrapped
//! — native_in (каждым блоком, per_block_crypto.rs). Решение 472: Twocrypto-NG и Tricrypto-NG — ещё состояние tweak_price
//! (load_crypto_tweak.rs) в той же пачке — для модели откатов после обмена (math/crypto_tweak2.rs).
//! Кто вызывает: pools/load.rs (ветка CurveTwocrypto / CurveTricrypto), pools/load_site.rs (пары многомонетного пула).
//! От чего зависит: ctx.rs, node/call.rs, node/multicall.rs, abi_crypto.rs, crypto_variant.rs, load_lb.rs (вызов времени блока),
//! load_smardex.rs (время блока без Multicall3), families.rs, per_block_crypto.rs (takes_native).
//! Настройки: dex.multicall3 [bsc_dex] (не задан — пачкой JSON-RPC и время блока eth_getBlockByNumber);
//! dex.curve_crypto_math, dex.curve_crypto_math_v2_0_0 [DEX] (версия формул по MATH()); dex.native_wrapped
//! (решение 455).
//!
//! Источник — строки геттеров в abi_crypto.rs; `_A_gamma` (рампа) — CurveTwocryptoOptimized.vy L1137-L1159,
//! CurveTricryptoOptimizedWETH.vy L1263-L1286.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{CryptoData, CryptoMath, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::{call, multicall};
use super::abi_crypto::{ICryptoPool as P, ITricryptoPool as P3, ITwocryptoPool as P2};
use super::crypto_variant;
use super::families::{Family, Proto};
use super::load_crypto_tweak;
use super::load_lb::{decode_time, time_call};
use super::load_smardex;
use super::per_block_crypto;

/// Вызовов до монет: A, gamma, D, mid_fee, out_fee, fee_gamma, future_A_gamma_time, precisions.
const HEAD: usize = 8;

/// Число монет пула по виду: Twocrypto — 2, Tricrypto — 3 (N_COINS исходников). None — вид не crypto. Ошибок нет.
pub fn coins_of(f: &Family) -> Option<usize> {
    match f.proto {
        Proto::CurveTwocrypto => Some(2),
        Proto::CurveTricrypto => Some(3),
        _ => None,
    }
}

/// Прочитанные ответы пула (решение 295: одно чтение на все пары многомонетного пула): пул, число монет, ответы,
/// время блока.
pub struct Raw {
    pub pool: Address,
    pub n: usize,
    pub got: Vec<Result<Bytes, String>>,
    pub now: u64,
}

/// Вызовы чтения пула по порядку: HEAD, затем coins(k), balances(k) по монетам, затем price_scale, последним MATH().
/// Ошибок нет.
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
    data.push(if n == 2 {
        P2::precisionsCall {}.abi_encode().into()
    } else {
        P3::precisionsCall {}.abi_encode().into()
    });
    for k in 0..n {
        let i = U256::from(k);
        data.push(P::coinsCall { i }.abi_encode().into());
        data.push(P::balancesCall { i }.abi_encode().into());
    }
    if n == 2 {
        data.push(P2::price_scaleCall {}.abi_encode().into());
    } else {
        for k in 0..n - 1 {
            let k = U256::from(k);
            data.push(P3::price_scaleCall { k }.abi_encode().into());
        }
    }
    data.push(P::MATHCall {}.abi_encode().into());
    data.into_iter().map(|d| (pool, d)).collect()
}

/// Что делает: сетевое чтение пула без пары. Принимает: окружение, вид, пул, номер блока. Возвращает: Raw.
/// Ошибки: вид не crypto, узел, ответов не столько, сколько вызовов, время блока не читается.
pub async fn read(ctx: &Ctx, f: &Family, pool: Address, block: u64) -> GatewayResult<Raw> {
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("Crypto {pool}: {what}"));
    let Some(n) = coins_of(f) else {
        return Err(bad(format!("{}: вид не crypto", f.title)));
    };
    let mut all = calls(pool, n);
    // Решение 472: состояние tweak_price — после вызовов calls (load_crypto_tweak.rs).
    all.extend(load_crypto_tweak::calls(pool, n));
    let mc = multicall::address(ctx);
    if let Some(m) = mc {
        all.push(time_call(m));
    }
    let tag = call::block_tag(block);
    let mut got = call::try_many(ctx, &all, &tag).await?;
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
    Ok(Raw { pool, n, got, now })
}

/// Состояние пары на блоке. Принимает: окружение, вид, пул, пару (token0, token1), блок. Ошибки: как read и state.
pub async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    block: u64,
) -> GatewayResult<PoolState> {
    state(ctx, f, &read(ctx, f, pool, block).await?, pair)
}

/// Что делает: состояние пары из прочитанного (без сети). Принимает: окружение (настройки и текст ошибки), вид, Raw,
/// пару. Возвращает: PoolState::Crypto. Ошибки: откат вызова, ответ не по ABI, MATH() не из списков настроек (формула
/// не сверена) или сразу в обоих, нет монеты пары среди монет пула; списки математики (crypto_variant::lists) —
/// NotConfigured (не задана dex.curve_crypto_math), Internal (не список, не адрес).
pub fn state(
    ctx: &Ctx,
    f: &Family,
    raw: &Raw,
    pair: (Address, Address),
) -> GatewayResult<PoolState> {
    let pool = raw.pool;
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("Crypto {pool}: {what}"));
    let (coins, mut data, math) = decode(raw).map_err(&bad)?;
    // Ревью 453: списки — crypto_variant::lists (не задана v2.0.0 — пусто; ошибка разбора — ошибка чтения с причиной).
    let (v200, ng) = crypto_variant::lists(ctx, f.id)?;
    data.math = crypto_variant::variant(raw.n, math, &v200, &ng).map_err(&bad)?;
    let index = |t: Address| {
        coins
            .iter()
            .position(|c| *c == t)
            .map(|p| p as u8)
            .ok_or_else(|| bad(format!("монеты {t} нет среди монет пула")))
    };
    let (i0, i1) = (index(pair.0)?, index(pair.1)?);
    data.i0 = i0;
    data.i1 = i1;
    // Решение 455: Tricrypto-NG с монетой dex.native_wrapped — нативный ETH без события (per_block_crypto.rs).
    data.native_in =
        f.proto == Proto::CurveTricrypto && per_block_crypto::takes_native(ctx, &coins);
    Ok(PoolState::Crypto(Box::new(data)))
}

/// Что делает: разбор ответов (без сети) — монеты пула, состояние (индексы пары — 0, версия — по числу монет: v2.1.0 /
/// Tricrypto-NG; точную ставит state по MATH()) и MATH() пула. Принимает: Raw. Возвращает: (монеты, CryptoData,
/// MATH()) или текст причины (откат вызова, ответ не по ABI).
pub fn decode(raw: &Raw) -> Result<(Vec<Address>, CryptoData, Address), String> {
    let got = &raw.got;
    let ok = |i: usize| match got.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!("вызов {i} откатился: {r}")),
        None => Err(format!("нет ответа на вызов {i}")),
    };
    let w = |i: usize| -> Result<U256, String> {
        let b = ok(i)?;
        let Some(v) = b.get(..32) else {
            return Err(format!("ответ {i} короче 32 байт"));
        };
        Ok(U256::from_be_slice(v))
    };
    let precisions = if raw.n == 2 {
        let p = P2::precisionsCall::abi_decode_returns(ok(7)?).map_err(|e| e.to_string())?;
        p.to_vec()
    } else {
        let p = P3::precisionsCall::abi_decode_returns(ok(7)?).map_err(|e| e.to_string())?;
        p.to_vec()
    };
    let mut coins = Vec::with_capacity(raw.n);
    let mut balances = Vec::with_capacity(raw.n);
    for k in 0..raw.n {
        let c = P::coinsCall::abi_decode_returns(ok(HEAD + 2 * k)?).map_err(|e| e.to_string())?;
        coins.push(c);
        balances.push(w(HEAD + 2 * k + 1)?);
    }
    let tail = HEAD + 2 * raw.n;
    let price_scale = (0..raw.n - 1)
        .map(|k| w(tail + k))
        .collect::<Result<Vec<_>, _>>()?;
    let math = P::MATHCall::abi_decode_returns(ok(tail + raw.n - 1)?).map_err(|e| e.to_string())?;
    // Решение 472: ответы состояния tweak_price — за MATH() (нет — запись без них: модель не применяется).
    let tweak = load_crypto_tweak::decode(got.get(tail + raw.n..).unwrap_or(&[]), raw.n)?;
    // Рампа A/gamma (`if t > block.timestamp`): обмен пересчитает D (newton_D), A и gamma меняются со временем (шапка).
    let ramp = w(6)? > U256::from(raw.now);
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
        math: if raw.n == 2 {
            CryptoMath::TwoNg210
        } else {
            CryptoMath::TriNg
        },
        ramp,
        moving: ramp,
        halted: false,
        native_in: false,
        i0: 0,
        i1: 0,
        tweak,
    };
    Ok((coins, data, math))
}
