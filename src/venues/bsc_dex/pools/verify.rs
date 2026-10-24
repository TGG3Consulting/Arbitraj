//! Файл: venues/bsc_dex/pools/verify.rs
//! Что делает: проверка пула из справочника индексатора (или из события создания) на своём узле перед тем, как его
//! слушать (решение 58; CLAUDE.md: «адрес контракта у биржи совпадает с адресом токена в пуле»). V2/V3: два прохода
//! Multicall3 — неизменяемые поля пула (factory, token0, token1; V3 — fee, tickSpacing; V2 со своей комиссией —
//! метод dex.v2_fee_method), затем реестр фабрики DEX (getPair / getPool) — именно он подтверждает, что это пул
//! этой фабрики (поддельный контракт может вернуть из factory() что угодно). V4/Infinity — verify_v4.rs.
//! Не совпало — причина текстом (её видно на экране «Монеты», в журнал — один раз).
//! Кто вызывает: pools/index.rs, pools/created.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_many), abi_check.rs, abi_v2v3.rs, discover.rs (Found), verify_rules.rs,
//! verify_v4.rs.
//! Настройки: dex.factory, dex.v2_fee_source, dex.v2_fee_method, dex.v2_fee_denominator [DEX]; dex.multicall3 [bsc_dex].
//!
//! ABI — abi_check.rs (ссылки там); getPair / getPool — abi_v2v3.rs (IUniswapV2Factory.sol#L9, IUniswapV3Factory.sol#L48).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::U24;
use alloy_primitives::{Address, Bytes, keccak256};

use crate::core::types::PoolRef;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_check::IPoolImmutables as P;
use super::abi_v2v3::{IV2Factory, IV3Factory};
use super::discover::Found;
use super::families::{Family, Proto};
pub use super::verify_rules::{Read, check, fill, registered, v2_fee_pips};
use super::verify_v4;

/// Кандидат на проверку: DEX (вид), пул как у источника (адрес или PoolId), монета и встречный токен.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub family: &'static Family,
    pub pool: String,
    pub coin: Address,
    pub quote: Address,
}

/// Проверка пачки кандидатов. Возвращает: по каждому (в том же порядке) найденный пул или причину отказа.
/// Ошибки: узел (вся пачка — «не проверено», не отказ).
pub async fn verify(ctx: &Ctx, cands: &[Candidate]) -> GatewayResult<Vec<Result<Found, String>>> {
    let mut out: Vec<Option<Result<Found, String>>> = vec![None; cands.len()];
    let (v4, v2v3): (Vec<usize>, Vec<usize>) = (0..cands.len()).partition(|i| {
        matches!(
            cands[*i].family.proto,
            Proto::InfinityCl | Proto::InfinityBin | Proto::UniswapV4
        )
    });
    let list: Vec<Candidate> = v4.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in v4.into_iter().zip(verify_v4::verify(ctx, &list).await?) {
        out[i] = Some(r);
    }
    let list: Vec<Candidate> = v2v3.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in v2v3.into_iter().zip(contracts(ctx, &list).await?) {
        out[i] = Some(r);
    }
    Ok(out.into_iter().flatten().collect())
}

/// Пулы V2/V3: поля пула, затем реестр фабрики. Ошибки: узел.
async fn contracts(ctx: &Ctx, cands: &[Candidate]) -> GatewayResult<Vec<Result<Found, String>>> {
    let mut reads: Vec<Result<(Address, Read), String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    let mut spans = Vec::new();
    for c in cands {
        let plan = Address::from_str(&c.pool)
            .map_err(|_| format!("«{}» — не адрес контракта пула", c.pool))
            .and_then(|a| field_calls(ctx, c.family).map(|l| (a, l)));
        match plan {
            Ok((a, list)) => {
                spans.push(Some((calls.len(), list.len())));
                calls.extend(list.into_iter().map(|d| (a, d)));
                reads.push(Ok((a, Read::default())));
            }
            Err(w) => {
                spans.push(None);
                reads.push(Err(w));
            }
        }
    }
    let got = call::try_many(ctx, &calls, "latest").await?;
    for ((c, r), span) in cands.iter().zip(reads.iter_mut()).zip(&spans) {
        if let (Ok((_, read)), Some((at, n))) = (&mut *r, span)
            && let Err(w) = fill(c.family, read, &got[*at..*at + *n])
        {
            *r = Err(w);
        }
    }
    // Второй проход — реестр фабрики по прочитанным токенам и комиссии.
    let mut reg = Vec::new();
    let mut checked: Vec<Result<(Address, Read), String>> = Vec::new();
    for (c, r) in cands.iter().zip(reads) {
        let r = r.and_then(|(a, read)| {
            let expected = ctx
                .dex_addr("dex.factory", c.family.id)
                .map_err(|e| e.message)?;
            check(c, expected, &read)?;
            reg.push((expected, registry_call(c.family, &read)));
            Ok((a, read))
        });
        checked.push(r);
    }
    let answers = call::try_many(ctx, &reg, "latest").await?;
    let mut answers = answers.into_iter();
    let mut out = Vec::new();
    for (c, r) in cands.iter().zip(checked) {
        out.push(r.and_then(|(a, read)| {
            let got = answers
                .next()
                .unwrap_or_else(|| Err("нет ответа реестра".into()))?;
            registered(a, &got)?;
            found(ctx, c, a, &read)
        }));
    }
    Ok(out)
}

/// Вызовы полей пула по виду. Ошибка — текст (не задан метод комиссии).
fn field_calls(ctx: &Ctx, f: &Family) -> Result<Vec<Bytes>, String> {
    let mut l = vec![
        Bytes::from(P::factoryCall {}.abi_encode()),
        Bytes::from(P::token0Call {}.abi_encode()),
        Bytes::from(P::token1Call {}.abi_encode()),
    ];
    match f.proto {
        Proto::V2 => {
            if let Some(sig) = fee_method(ctx, f)? {
                l.push(Bytes::copy_from_slice(&keccak256(sig.as_bytes())[..4]));
            }
        }
        _ => {
            l.push(Bytes::from(P::feeCall {}.abi_encode()));
            l.push(Bytes::from(P::tickSpacingCall {}.abi_encode()));
        }
    }
    Ok(l)
}

/// Метод своей комиссии пары V2: Some(сигнатура) при dex.v2_fee_source = pool, иначе None. Ошибка — не задано.
fn fee_method(ctx: &Ctx, f: &Family) -> Result<Option<String>, String> {
    match ctx
        .dex_text("dex.v2_fee_source", f.id)
        .map_err(|e| e.message)?
        .as_str()
    {
        "pool" => Ok(Some(
            ctx.dex_text("dex.v2_fee_method", f.id)
                .map_err(|e| e.message)?
                .trim()
                .to_string(),
        )),
        _ => Ok(None),
    }
}

/// Вызов реестра фабрики: V2 getPair(token0, token1), V3 getPool(token0, token1, fee). Ошибок нет.
fn registry_call(f: &Family, r: &Read) -> Bytes {
    match f.proto {
        Proto::V2 => Bytes::from(
            IV2Factory::getPairCall {
                tokenA: r.token0,
                tokenB: r.token1,
            }
            .abi_encode(),
        ),
        _ => Bytes::from(
            IV3Factory::getPoolCall {
                tokenA: r.token0,
                tokenB: r.token1,
                fee: U24::from(r.fee.unwrap_or_default()),
            }
            .abi_encode(),
        ),
    }
}

/// Найденный пул V2/V3 по прочитанному. Ошибка — не задан делитель комиссии или комиссия неверна.
fn found(ctx: &Ctx, c: &Candidate, pool: Address, r: &Read) -> Result<Found, String> {
    let fee_pips = match (c.family.proto, r.v2_fee) {
        (Proto::V2, Some(v)) => {
            let den = ctx
                .dex_int("dex.v2_fee_denominator", c.family.id)
                .map_err(|e| e.message)?;
            Some(v2_fee_pips(v, den)?)
        }
        (Proto::V2, None) => None,
        _ => r.fee,
    };
    Ok(Found {
        family: c.family,
        reference: PoolRef::Contract(pool),
        token0: r.token0,
        token1: r.token1,
        coin: c.coin,
        fee_pips,
        tick_spacing: r.spacing,
        bin_step: None,
        key: None,
        hooks: None,
    })
}
