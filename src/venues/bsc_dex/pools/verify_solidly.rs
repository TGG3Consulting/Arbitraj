//! Файл: venues/bsc_dex/pools/verify_solidly.rs
//! Что делает: проверка пула Solidly (ve(3,3)-форки: Topaz v2, THENA V1; решения 80, 85) из справочника индексатора
//! на своём узле — одним проходом Multicall3: реестр фабрики `getPool(coin, quote, stable)` должен вернуть этот пул
//! (решение 59 «своя фабрика подтверждает пул»; CLAUDE.md: адрес токена у биржи = адрес в пуле). Флаг stable пула
//! заранее неизвестен, а реестр фабрики заполнен в обе стороны токенов (PoolFactory.sol), поэтому спрашиваем ОБА
//! варианта stable (true и false) и принимаем пул, если хотя бы один вернул именно его адрес — это подтверждает и
//! принадлежность фабрике, и пару. Сам флаг stable (и ориентацию) читает load_solidly.rs из metadata().
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_solidly.rs, discover.rs (Found, sorted), verify.rs.
//! Настройки: dex.factory [DEX] (фабрика Solidly).
//!
//! getPool(tokenA, tokenB, bool stable) → address — PoolFactory.sol L68-L71 (реестр `_getPool`, заполнен в обе
//! стороны L153-L154), velodrome-finance HEAD b3065d8 (решение 23).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_solidly::ISolidlyFactory;
use super::discover::{Found, sorted};
use super::verify::{Candidate, Verdict};

/// Проверка пачки кандидатов Solidly частями по `size` вызовов (по 2 вызова на кандидата: getPool для stable=true и
/// stable=false). Возвращает: итог по каждому (сбой узла на части — Failed). Ошибок нет.
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<Address, String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = (|| {
            let pool = Address::from_str(&c.pool)
                .map_err(|_| format!("«{}» — не адрес контракта пула Solidly", c.pool))?;
            let factory = ctx
                .dex_addr("dex.factory", c.family.id)
                .map_err(|e| e.message)?;
            let one = |stable: bool| {
                (
                    factory,
                    Bytes::from(
                        ISolidlyFactory::getPoolCall {
                            tokenA: c.coin,
                            tokenB: c.quote,
                            stable,
                        }
                        .abi_encode(),
                    ),
                )
            };
            Ok((pool, one(true), one(false)))
        })();
        match plan {
            Ok((pool, call_t, call_f)) => {
                calls.push(call_t);
                calls.push(call_f);
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
        // Два ответа подряд: stable=true, stable=false.
        let a_true = got.next();
        let a_false = got.next();
        out.push(decide(c, pool, a_true, a_false));
    }
    out
}

/// Итог по двум ответам реестра (stable=true и stable=false), без сети. Пул принят, если любой ответ — его адрес.
/// Сбой узла хоть в одном ответе — Failed. Ошибок нет (причина — текст для интерфейса).
fn decide(
    c: &Candidate,
    pool: Address,
    a_true: Option<call::Piece>,
    a_false: Option<call::Piece>,
) -> Verdict {
    let one = |ans: Option<call::Piece>| -> Result<bool, Verdict> {
        match ans {
            None => Err(Verdict::Failed("нет ответа реестра фабрики Solidly".into())),
            Some(Err(node)) => Err(Verdict::Failed(node)),
            Some(Ok(Err(rev))) => Err(Verdict::Rejected(format!(
                "реестр фабрики Solidly откатился: {rev}"
            ))),
            Some(Ok(Ok(b))) => match ISolidlyFactory::getPoolCall::abi_decode_returns(&b) {
                Ok(addr) => Ok(addr == pool),
                Err(e) => Err(Verdict::Rejected(format!(
                    "ответ getPool фабрики Solidly не по ABI: {e}"
                ))),
            },
        }
    };
    let hit_true = match one(a_true) {
        Ok(h) => h,
        Err(v) => return v,
    };
    let hit_false = match one(a_false) {
        Ok(h) => h,
        Err(v) => return v,
    };
    if !hit_true && !hit_false {
        return Verdict::Rejected(format!(
            "фабрика Solidly (dex.factory DEX {}) не знает этот пул для пары {} / {}",
            c.family.id, c.coin, c.quote
        ));
    }
    let (token0, token1) = sorted(c.coin, c.quote);
    Verdict::Taken(Box::new(Found {
        family: c.family,
        reference: PoolRef::Contract(pool),
        token0,
        token1,
        coin: c.coin,
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    }))
}
