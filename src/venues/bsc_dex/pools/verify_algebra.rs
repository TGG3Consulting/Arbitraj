//! Файл: venues/bsc_dex/pools/verify_algebra.rs
//! Что делает: проверка пула Algebra (THENA Fusion и форки Algebra V1/V1.9, решения 80, 81) из справочника индексатора
//! на своём узле — одним проходом Multicall3: фабрика Algebra (poolByPair(coin, quote)) должна вернуть именно этот пул
//! (решение 59 «кто меняет / своя фабрика»; CLAUDE.md: адрес токена у биржи = адрес в пуле). У Algebra на пару токенов
//! ровно ОДИН пул (комиссия динамическая, уровней комиссии нет), поэтому poolByPair однозначно подтверждает подлинность
//! пула: поддельный контракт с нужными геттерами фабрика по этой паре не вернёт. Детерминированная сверка адреса по
//! POOL_INIT_CODE_HASH/poolDeployer (CREATE2) — возможное усиление позже; в фазе B достаточно авторитетного реестра
//! самой фабрики (как у V2/V3/Elastic/DODO).
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_algebra.rs, discover.rs (Found, sorted), verify.rs.
//! Настройки: dex.factory [DEX] (фабрика Algebra, например THENA Fusion 0x30055F87716d3DFD0E5198C27024481099fB4A98).
//!
//! poolByPair(tokenA, tokenB) → address — IAlgebraFactory.sol (@cryptoalgebra/v1.9-core, решение 23): порядок токенов
//! не важен, фабрика сама их сортирует; нулевой адрес — пары нет.

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_algebra::IAlgebraFactory;
use super::discover::{Found, sorted};
use super::verify::{Candidate, Verdict};

/// Проверка пачки кандидатов Algebra частями по `size` вызовов. Возвращает: итог по каждому (сбой узла на части —
/// Failed). Ошибок нет.
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<Address, String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = (|| {
            let pool = Address::from_str(&c.pool)
                .map_err(|_| format!("«{}» — не адрес контракта пула Algebra", c.pool))?;
            let factory = ctx
                .dex_addr("dex.factory", c.family.id)
                .map_err(|e| e.message)?;
            let data = IAlgebraFactory::poolByPairCall {
                tokenA: c.coin,
                tokenB: c.quote,
            }
            .abi_encode();
            Ok((pool, factory, Bytes::from(data)))
        })();
        match plan {
            Ok((pool, factory, data)) => {
                calls.push((factory, data));
                plans.push(Ok(pool));
            }
            Err(w) => plans.push(Err(w)),
        }
    }
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut got = got.into_iter();
    let mut out = Vec::new();
    for (c, plan) in cands.iter().zip(plans) {
        let Ok(pool) = plan else {
            out.push(Verdict::Rejected(plan.err().unwrap_or_default()));
            continue;
        };
        out.push(match got.next() {
            None => Verdict::Failed("нет ответа фабрики Algebra".into()),
            Some(Err(node)) => Verdict::Failed(node),
            Some(Ok(Err(rev))) => Verdict::Rejected(format!("фабрика Algebra откатилась: {rev}")),
            Some(Ok(Ok(b))) => match decide(c, pool, &b) {
                Ok(f) => Verdict::Taken(Box::new(f)),
                Err(w) => Verdict::Rejected(w),
            },
        });
    }
    out
}

/// Решение по ответу фабрики (без сети). Принимает: кандидата, адрес пула, ответ poolByPair.
/// Возвращает: найденный пул или причину отказа.
pub fn decide(c: &Candidate, pool: Address, answer: &[u8]) -> Result<Found, String> {
    let got = IAlgebraFactory::poolByPairCall::abi_decode_returns(answer)
        .map_err(|e| format!("ответ фабрики Algebra не по ABI: {e}"))?;
    if got != pool {
        return Err(format!(
            "фабрика Algebra (dex.factory DEX {}) для пары {} / {} даёт пул {got}, а не {pool}",
            c.family.id, c.coin, c.quote
        ));
    }
    let (token0, token1) = sorted(c.coin, c.quote);
    Ok(Found {
        family: c.family,
        reference: PoolRef::Contract(pool),
        token0,
        token1,
        coin: c.coin,
        // Комиссия Algebra динамическая — её текущее значение читает load_algebra.rs (globalState.fee).
        fee_pips: None,
        // tickSpacing пулу Algebra не нужен: битмап индексирует сырой тик (load_algebra.rs, TickMap.spacing = 1).
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    })
}
