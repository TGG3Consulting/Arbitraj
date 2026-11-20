//! Файл: venues/bsc_dex/pools/load_site.rs
//! Что делает: решение 295 — состояние многомонетного пула читается ОДИН раз на контракт для всех его пар
//! (`PoolRef::Multi`): одно сетевое чтение загрузчика вида (load_curve / load_saddle / load_curve_ng / load_stable_u /
//! load_balancer — `read`), затем сборка состояния каждой пары без сети (`state` — индексы пары среди монет пула).
//! Группы (контракт → пары) для пачечного чтения — `groups` (load_batch.rs). Ссылки Contract / Singleton / Member сюда
//! не попадают: их чтение — прежнее (load.rs, load_batch.rs).
//! Кто вызывает: pools/load_batch.rs (states; через него — reload_many.rs load_group: per_block.rs, listen_site.rs,
//! приём и повтор пачки).
//! От чего зависит: load.rs (Loaded), families.rs, load_curve.rs, load_curve_plain.rs (решение 472), load_saddle.rs,
//! load_curve_ng.rs, load_stable_u.rs, load_balancer.rs, load_curve_oracle.rs (решение 481), load_wombat.rs (решение
//! 484), tokens.rs, node/call.rs
//! (block_tag).
//! Настройки: через загрузчики — dex.vault, dex.factory (Balancer), dex.native_wrapped (PancakeSwap StableSwap, Curve
//! Plain*ETH).

use std::collections::HashMap;

use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call::block_tag;
use super::super::tokens::Tokens;
use super::families::{Family, Proto};
use super::family_variant::Variant;
use super::load::Loaded;
use super::load_batch::Want;
use super::{
    load_balancer, load_balancer_v3, load_crypto, load_crypto_v1, load_curve, load_curve_meta,
    load_curve_ng, load_curve_oracle, load_curve_plain, load_saddle, load_stable_u, load_wombat,
};

/// Что делает: состояния всех пар одного контракта на блоке одним чтением. Принимает: окружение, токены (decimals),
/// вид DEX, контракт пула, пары (token0, token1), блок. Возвращает: по каждой паре Loaded или причину; сбой чтения —
/// одна и та же причина у всех пар. Ошибок иных нет.
pub async fn states(
    ctx: &Ctx,
    tokens: &Tokens,
    f: &Family,
    contract: Address,
    pairs: &[(Address, Address)],
    block: u64,
) -> Vec<GatewayResult<Loaded>> {
    let tag = block_tag(block);
    let built: Vec<GatewayResult<PoolState>> = match f.proto {
        // Решение 472: вариант Curve 2021 — A_precise() и свой шаблон (load_curve_plain.rs).
        // Решение 472 (доделка): метапул — курс LP базового пула (load_curve_meta.rs).
        Proto::CurveStable if f.variant == Variant::CurveMeta => {
            match load_curve_meta::read(ctx, tokens, contract, &tag).await {
                Ok(raw) => pairs.iter().map(|p| load_curve_meta::state(ctx, &raw, *p)).collect(),
                Err(e) => pairs.iter().map(|_| Err(e.clone())).collect(),
            }
        }
        // Решение 481: пул с курсом оракула (load_curve_oracle.rs).
        Proto::CurveStable if f.variant == Variant::CurveOracle => {
            match load_curve_oracle::read(ctx, tokens, contract, &tag).await {
                Ok(raw) => pairs.iter().map(|p| load_curve_oracle::state(ctx, &raw, *p)).collect(),
                Err(e) => pairs.iter().map(|_| Err(e.clone())).collect(),
            }
        }
        Proto::CurveStable if f.variant == Variant::CurvePlain => {
            match load_curve_plain::read(ctx, tokens, contract, &tag).await {
                Ok(raw) => pairs.iter().map(|p| load_curve_plain::state(ctx, &raw, *p)).collect(),
                Err(e) => pairs.iter().map(|_| Err(e.clone())).collect(),
            }
        }
        Proto::CurveStable => match load_curve::read(ctx, tokens, contract, &tag).await {
            Ok(raw) => pairs.iter().map(|p| load_curve::state(ctx, &raw, *p)).collect(),
            Err(e) => pairs.iter().map(|_| Err(e.clone())).collect(),
        },
        Proto::SaddleStable => match load_saddle::read(ctx, tokens, contract, &tag).await {
            Ok(raw) => pairs.iter().map(|p| load_saddle::state(ctx, &raw, *p)).collect(),
            Err(e) => pairs.iter().map(|_| Err(e.clone())).collect(),
        },
        Proto::CurveNg => match load_curve_ng::read(ctx, contract, &tag).await {
            Ok(raw) => pairs.iter().map(|p| load_curve_ng::state(ctx, &raw, *p)).collect(),
            Err(e) => pairs.iter().map(|_| Err(e.clone())).collect(),
        },
        // Curve Tricrypto-NG (решение 442): одно чтение пула и время блока на все пары (load_crypto.rs).
        Proto::CurveTricrypto => match load_crypto::read(ctx, f, contract, block).await {
            Ok(raw) => pairs.iter().map(|p| load_crypto::state(ctx, f, &raw, *p)).collect(),
            Err(e) => pairs.iter().map(|_| Err(e.clone())).collect(),
        },
        // Tricrypto 2021 (решение 453): одно чтение пула на все пары (load_crypto_v1.rs).
        Proto::CurveTricryptoV1 => match load_crypto_v1::read(ctx, tokens, f, contract, block).await {
            Ok(raw) => pairs.iter().map(|p| load_crypto_v1::state(ctx, &raw, *p)).collect(),
            Err(e) => pairs.iter().map(|_| Err(e.clone())).collect(),
        },
        // Решение 484: Wombat — одно чтение пула на все пары (load_wombat.rs).
        Proto::Wombat => match load_wombat::read(ctx, f, contract, &tag).await {
            Ok(raw) => pairs.iter().map(|p| load_wombat::state(ctx, &raw, *p)).collect(),
            Err(e) => pairs.iter().map(|_| Err(e.clone())).collect(),
        },
        Proto::PancakeStable => match pancake_read(ctx, tokens, contract, &tag).await {
            Ok(raw) => pairs.iter().map(|p| pancake_state(ctx, &raw, *p)).collect(),
            Err(e) => pairs.iter().map(|_| Err(e.clone())).collect(),
        },
        Proto::BalancerV2 => match load_balancer::read(ctx, f, contract, &tag).await {
            Ok((fx, got)) => pairs
                .iter()
                .map(|p| {
                    load_balancer::decode(&fx, &got, *p).map_err(|w| {
                        ctx.err(ErrorCode::BadResponse, format!("Balancer {contract}: {w}"))
                    })
                })
                .collect(),
            Err(e) => pairs.iter().map(|_| Err(e.clone())).collect(),
        },
        // Решение 441: Balancer V3 — одно чтение на пул, разбор по паре (load_balancer_v3.rs).
        Proto::BalancerV3 => match load_balancer_v3::read(ctx, f, contract, &tag).await {
            Ok((fx, got)) => pairs
                .iter()
                .map(|p| {
                    load_balancer_v3::decode(contract, &fx, &got, *p).map_err(|w| {
                        ctx.err(ErrorCode::BadResponse, format!("Balancer V3 {contract}: {w}"))
                    })
                })
                .collect(),
            Err(e) => pairs.iter().map(|_| Err(e.clone())).collect(),
        },
        _ => pairs
            .iter()
            .map(|_| {
                Err(ctx.err(
                    ErrorCode::Internal,
                    format!("{}: вид пула не многомонетный (решение 295)", f.title),
                ))
            })
            .collect(),
    };
    built
        .into_iter()
        .map(|r| {
            r.map(|state| Loaded {
                state,
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        })
        .collect()
}

/// Что делает: группы пачки по контракту многомонетного пула — индексы Want с одной ссылкой Multi одного вида DEX на
/// один контракт (порядок — первого появления). Want с иной ссылкой в группы не входят. Принимает: Want пачки.
/// Возвращает: (контракт, индексы Want группы). Ошибок нет.
pub fn groups(wants: &[Want<'_>]) -> Vec<(Address, Vec<usize>)> {
    let mut at: HashMap<(&'static str, Address), usize> = HashMap::new();
    let mut out: Vec<(Address, Vec<usize>)> = Vec::new();
    for (i, w) in wants.iter().enumerate() {
        let PoolRef::Multi { contract, .. } = w.reference else {
            continue;
        };
        match at.get(&(w.family.id, *contract)) {
            Some(&g) => out[g].1.push(i),
            None => {
                at.insert((w.family.id, *contract), out.len());
                out.push((*contract, vec![i]));
            }
        }
    }
    out
}

/// Прочитанный пул PancakeSwap StableSwap: ответы, индекс fee(), монеты, балансы, множители точности.
pub struct PancakeRaw {
    pub pool: Address,
    pub got: Vec<Result<Bytes, String>>,
    pub tail: usize,
    pub coins: Vec<Address>,
    pub balances: Vec<U256>,
    pub rates: Vec<U256>,
}

/// Что делает: сетевое чтение пула PancakeSwap StableSwap без пары — те же вызовы и части, что load_stable_u::load
/// (answers, head, rates). Принимает: окружение, токены, пул, блок-тег. Возвращает: PancakeRaw.
/// Ошибки: как у load_stable_u (узел, ответ не по ABI, decimals > 36).
pub async fn pancake_read(ctx: &Ctx, tokens: &Tokens, pool: Address, tag: &str) -> GatewayResult<PancakeRaw> {
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("StableSwap {pool}: {what}"));
    let (got, tail) = load_stable_u::answers(ctx, pool, false, tag).await?;
    let (coins, balances) = load_stable_u::head(&got, &bad)?;
    let rates = load_stable_u::rates(ctx, tokens, &coins, &bad).await?;
    Ok(PancakeRaw { pool, got, tail, coins, balances, rates })
}

/// Что делает: состояние пары пула PancakeSwap StableSwap из прочитанного (без сети): индексы пары и сборка
/// load_stable_u::finish. Принимает: окружение, PancakeRaw, пару. Ошибки: монеты пары нет, откат обязательного вызова.
pub fn pancake_state(ctx: &Ctx, raw: &PancakeRaw, pair: (Address, Address)) -> GatewayResult<PoolState> {
    let pool = raw.pool;
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("StableSwap {pool}: {what}"));
    let ix = load_stable_u::indices(ctx, &raw.coins, pair, &bad)?;
    load_stable_u::finish(
        &raw.got,
        raw.tail,
        false,
        &raw.coins,
        raw.balances.clone(),
        raw.rates.clone(),
        ix,
        &bad,
    )
}
