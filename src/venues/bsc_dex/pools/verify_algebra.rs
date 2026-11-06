//! Файл: venues/bsc_dex/pools/verify_algebra.rs
//! Что делает: проверка пула Algebra (THENA Fusion и форки Algebra V1/V1.9, решения 80, 81) из справочника индексатора
//! на своём узле — одним проходом Multicall3: фабрика Algebra (poolByPair(coin, quote)) должна вернуть именно этот пул
//! (решение 59 «кто меняет / своя фабрика»; CLAUDE.md: адрес токена у биржи = адрес в пуле). У Algebra на пару токенов
//! ровно ОДИН пул (комиссия динамическая, уровней комиссии нет), поэтому poolByPair однозначно подтверждает подлинность
//! пула: поддельный контракт с нужными геттерами фабрика по этой паре не вернёт. Детерминированная сверка адреса по
//! POOL_INIT_CODE_HASH/poolDeployer (CREATE2) — возможное усиление позже; в фазе B достаточно авторитетного реестра
//! самой фабрики (как у V2/V3/Elastic/DODO).
//! Решение 188 (Algebra Integral): у Integral кроме пула по умолчанию бывают КАСТОМНЫЕ пулы той же пары — их фабрика
//! хранит отдельно, по адресу деплоера (customPoolByPair(deployer, tokenA, tokenB), IAlgebraFactory.sol тега
//! v1.2.1-integral L137; событие CustomPool(deployer, token0, token1, pool) L36). У THENA V3 крупные пулы — кастомные
//! (живая запись 06.11.2026: poolByPair даёт другой адрес). Список деплоеров — dex.custom_deployers [DEX]: пул
//! принимается, если его вернул poolByPair ИЛИ customPoolByPair хоть одного деплоера списка. Не задан — как раньше
//! (только poolByPair).
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_algebra.rs, discover.rs (Found, sorted), verify.rs.
//! Настройки: dex.custom_deployers [DEX] (необязательная, Algebra Integral); dex.factory [DEX] (фабрика Algebra, например THENA Fusion 0x30055F87716d3DFD0E5198C27024481099fB4A98).
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
use super::abi_algebra_integral::IAlgebraIntegralFactory;
use super::discover::{Found, sorted};
use super::families::Proto;
use super::verify::{Candidate, Verdict};

/// Проверка пачки кандидатов Algebra частями по `size` вызовов. Возвращает: итог по каждому (сбой узла на части —
/// Failed). Ошибок нет.
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    // По каждому кандидату — пул и сколько вызовов фабрики (poolByPair + customPoolByPair деплоеров).
    let mut plans: Vec<Result<(Address, usize), String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        match plan(ctx, c) {
            Ok((pool, list)) => {
                plans.push(Ok((pool, list.len())));
                calls.extend(list);
            }
            Err(w) => plans.push(Err(w)),
        }
    }
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut got = got.into_iter();
    let mut out = Vec::new();
    for (c, plan) in cands.iter().zip(plans) {
        let (pool, n) = match plan {
            Ok(v) => v,
            Err(w) => {
                out.push(Verdict::Rejected(w));
                continue;
            }
        };
        let answers: Vec<_> = got.by_ref().take(n).collect();
        out.push(judge(c, pool, answers));
    }
    out
}

/// Вызовы фабрики для кандидата: poolByPair(coin, quote) и, у Algebra Integral с dex.custom_deployers,
/// customPoolByPair(деплоер, coin, quote) по каждому. Ошибки (текст): не адрес пула, нет dex.factory, деплоер не адрес.
fn plan(ctx: &Ctx, c: &Candidate) -> Result<(Address, Vec<(Address, Bytes)>), String> {
    let pool = Address::from_str(&c.pool)
        .map_err(|_| format!("«{}» — не адрес контракта пула Algebra", c.pool))?;
    let factory = ctx
        .dex_addr("dex.factory", c.family.id)
        .map_err(|e| e.message)?;
    let mut list = vec![(
        factory,
        Bytes::from(
            IAlgebraFactory::poolByPairCall {
                tokenA: c.coin,
                tokenB: c.quote,
            }
            .abi_encode(),
        ),
    )];
    if c.family.proto == Proto::AlgebraIntegral {
        for d in ctx
            .dex_list("dex.custom_deployers", c.family.id)
            .unwrap_or_default()
        {
            let deployer = Address::from_str(d.trim())
                .map_err(|_| format!("dex.custom_deployers: «{d}» — не адрес"))?;
            let data = IAlgebraIntegralFactory::customPoolByPairCall {
                customDeployer: deployer,
                tokenA: c.coin,
                tokenB: c.quote,
            }
            .abi_encode();
            list.push((factory, Bytes::from(data)));
        }
    }
    Ok((pool, list))
}

/// Итог по ответам фабрики: первый ответ — poolByPair (решает decide), остальные — customPoolByPair деплоеров
/// (любой, вернувший этот пул, подтверждает его). Сбой узла по poolByPair — Failed. Ошибок нет.
fn judge(
    c: &Candidate,
    pool: Address,
    answers: Vec<Result<Result<Bytes, String>, String>>,
) -> Verdict {
    let mut it = answers.into_iter();
    let first = match it.next() {
        None => return Verdict::Failed("нет ответа фабрики Algebra".into()),
        Some(Err(node)) => return Verdict::Failed(node),
        Some(Ok(Err(rev))) => {
            return Verdict::Rejected(format!("фабрика Algebra откатилась: {rev}"));
        }
        Some(Ok(Ok(b))) => b,
    };
    let custom = it.any(|a| {
        matches!(a, Ok(Ok(b)) if IAlgebraIntegralFactory::customPoolByPairCall::abi_decode_returns(&b)
            .is_ok_and(|got| got == pool))
    });
    match decide(c, pool, &first) {
        Ok(f) => Verdict::Taken(Box::new(f)),
        Err(_) if custom => Verdict::Taken(Box::new(found(c, pool))),
        Err(w) => Verdict::Rejected(w),
    }
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
    Ok(found(c, pool))
}

/// Подтверждённый пул Algebra. Ошибок нет.
fn found(c: &Candidate, pool: Address) -> Found {
    let (token0, token1) = sorted(c.coin, c.quote);
    Found {
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
    }
}
