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
//! Решение 472 (T472-03): у пула фабрики (2 монеты) — ещё состояние tweak_price / _claim_admin_fees
//! (load_crypto_tweak_v1.rs): вызовы — в той же пачке после прежних (разбор прежних не меняется), слот not_adjusted и
//! ETH пула — параллельной пачкой JSON-RPC на том же блоке; ответы дописываются в конец Raw.got.
//! Кто вызывает: pools/load.rs, pools/load_site.rs (пары tricrypto).
//! От чего зависит: ctx.rs, node/call.rs, node/multicall.rs, tokens.rs, abi_crypto.rs, abi_crypto_v1.rs, load_lb.rs
//! (время блока Multicall3), load_smardex.rs (время блока без Multicall3), families.rs, per_block_crypto.rs
//! (takes_native, решение 455).
//! Настройки: dex.multicall3 [bsc_dex] (как load_crypto.rs); dex.native_wrapped (решение 455).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::{call, multicall};
use super::super::tokens::Tokens;
use super::abi_crypto::{ICryptoPool as P, ITricryptoPool as P3};
use super::abi_crypto_v1::ICryptoPoolV1 as V1;
use super::families::{Family, Proto};
use super::family_variant::Variant;
use super::load_crypto_tweak_list as tweak_list;
use super::load_crypto_tweak_v1 as tweak_v1;
use super::load_lb::{decode_time, time_call};
use super::load_smardex;
use super::per_block_crypto;

/// Разбор ответов — load_crypto_v1_decode.rs (вынесен; путь прежний).
pub use super::load_crypto_v1_decode::decode;

/// Вызовов до монет: A, gamma, D, mid_fee, out_fee, fee_gamma, future_A_gamma_time.
pub(super) const HEAD: usize = 7;

/// Прочитанное: пул, число монет, ответы, время блока, decimals монет (по порядку монет пула).
pub struct Raw {
    pub pool: Address,
    pub n: usize,
    pub got: Vec<Result<Bytes, String>>,
    pub now: u64,
    pub decimals: Vec<u8>,
    /// Читался is_killed() (последним вызовом после price_scale): пулы протокола tricrypto 2021 — tricrypto и пара
    /// без фабрики (вариант CryptoPair, решение 472).
    pub killed: bool,
}

/// Число монет вида: фабрика v1 — 2, tricrypto 2021 — 3. None — не вид v1. Ошибок нет.
pub fn coins_of(f: &Family) -> Option<usize> {
    match f.proto {
        Proto::CurveCryptoV1 => Some(2),
        // Решение 472 (T472-10): вариант CryptoPair — две монеты (EURS/2CRV).
        Proto::CurveTricryptoV1 if f.variant == Variant::CryptoPair => Some(2),
        Proto::CurveTricryptoV1 => Some(3),
        _ => None,
    }
}

/// Вызовы чтения по порядку: HEAD, coins(k)/balances(k), price_scale, у трёх монет — is_killed(). Ошибок нет.
pub fn calls(pool: Address, n: usize) -> Vec<(Address, Bytes)> {
    calls_with(pool, n, n == 3)
}

/// То же с is_killed() последним при `killed` (решение 472: пара без фабрики — CryptoPair). Ошибок нет.
pub fn calls_with(pool: Address, n: usize, killed: bool) -> Vec<(Address, Bytes)> {
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
    }
    if killed {
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
    let tag = call::block_tag(block);
    let killed = f.proto == Proto::CurveTricryptoV1;
    let mut all = calls_with(pool, n, killed);
    // T472-03: tweak_price — постоянные пула из памяти или одним чтением на этом блоке; пока слот not_adjusted не
    // подтверждён записью — без модели (причина — в журнал один раз на пул).
    let v1 = f.proto == Proto::CurveCryptoV1 && tweak_v1::CONFIRMED;
    let list = killed && tweak_list::CONFIRMED;
    if f.proto == Proto::CurveCryptoV1 && !v1 {
        tweak_list::unconfirmed(ctx, pool, "crypto v1 фабрики (слот 33)");
    } else if killed && !list {
        let what = format!(
            "пул без фабрики из {n} монет (слот {})",
            tweak_list::slot(n)
        );
        tweak_list::unconfirmed(ctx, pool, &what);
    }
    if v1 {
        all.extend(tweak_v1::calls(
            pool,
            &tweak_v1::statics(ctx, pool, &tag).await?,
        ));
    }
    if list {
        let (token, coins) = tweak_list::statics(ctx, pool, n, &tag).await?;
        all.extend(tweak_list::calls(pool, n, token, &coins));
    }
    let mc = multicall::address(ctx);
    if let Some(m) = mc {
        all.push(time_call(m));
    }
    // Пачка вызовов и пачка JSON-RPC tweak — параллельно, на одном блоке.
    let words = async {
        if v1 {
            tweak_v1::words(ctx, pool, &tag).await
        } else if list {
            tweak_list::words(ctx, pool, n, &tag).await
        } else {
            Ok(Vec::new())
        }
    };
    let (got, words) = tokio::join!(call::try_many(ctx, &all, &tag), words);
    let mut got = got?;
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
    // T472-03: слово not_adjusted и ETH пула — в конец ответов (load_crypto_tweak_v1.rs decode).
    got.extend(words?);
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
        killed,
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
    // Решение 455: crypto v1 фабрики (2 монеты; у tricrypto 2021 self.balance не читается) с монетой
    // dex.native_wrapped — нативный ETH без события (per_block_crypto.rs).
    data.native_in = raw.n == 2 && !raw.killed && per_block_crypto::takes_native(ctx, &coins);
    // T472-03: состояние tweak_price пула фабрики — ответы после прежних (HEAD, монеты, price_scale).
    if raw.n == 2 && !raw.killed {
        let tail = raw.got.get(HEAD + 2 * raw.n + 1..).unwrap_or(&[]);
        let eth = tweak_v1::remembered(ctx.place.id, pool).and_then(|s| s.eth);
        // Ревью 472 (п. 6): пул с монетой WETH фабрики — нужна dex.native_wrapped (load_crypto_tweak_v1.rs).
        if !tail.is_empty() {
            tweak_v1::need_wrapped(ctx, pool, &coins, eth)?;
        }
        data.tweak = tweak_v1::decode(tail, &coins, eth).map_err(&bad)?;
    }
    // T472-03: пулы без фабрики — ответы после is_killed (load_crypto_tweak_list.rs).
    if raw.killed {
        let tail = raw.got.get(HEAD + 2 * raw.n + raw.n..).unwrap_or(&[]);
        data.tweak = tweak_list::decode(tail, raw.n, raw.now).map_err(&bad)?;
    }
    Ok(PoolState::Crypto(Box::new(data)))
}
