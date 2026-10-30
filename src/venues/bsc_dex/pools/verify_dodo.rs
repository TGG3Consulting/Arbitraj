//! Файл: venues/bsc_dex/pools/verify_dodo.rs
//! Что делает: проверка пула DODO PMM (DVM/DPP/DSP, решения 80, 86b) из справочника индексатора на своём узле —
//! одним проходом Multicall3: реестр фабрики DODO (getDODOPoolBidirection(coin, quote)) должен содержать этот пул
//! (решение 59 «кто меняет / своя фабрика»; CLAUDE.md: адрес токена у биржи = адрес в пуле). Фабрика у DODO хранит
//! реестр base → quote → пулы, поэтому именно она подтверждает, что это настоящий DVM/DPP/DSP (поддельный контракт
//! с нужными геттерами реестр не знает). Ориентацию base/quote (нужна математике) читает уже load_dodo.rs по
//! _BASE_TOKEN_/_QUOTE_TOKEN_; здесь — только членство в реестре.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_dodo.rs, discover.rs (Found, sorted), verify.rs.
//! Настройки: dex.factory [DEX] (фабрика DVM/DPP/DSP).
//!
//! getDODOPoolBidirection(token0, token1) → (_REGISTRY_[token0][token1], _REGISTRY_[token1][token0]) —
//! Factory/DVMFactory.sol (и DPP/DSP, идентично), DODOEX/contractV2 (решение 23).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_dodo::IDodoFactory;
use super::discover::{Found, sorted};
use super::verify::{Candidate, Verdict};

/// Проверка пачки кандидатов DODO частями по `size` вызовов. Возвращает: итог по каждому (сбой узла на части —
/// Failed). Ошибок нет.
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<Address, String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = (|| {
            let pool = Address::from_str(&c.pool)
                .map_err(|_| format!("«{}» — не адрес контракта пула DODO", c.pool))?;
            let factory = ctx
                .dex_addr("dex.factory", c.family.id)
                .map_err(|e| e.message)?;
            // Реестр хранит base → quote; спрашиваем по паре (монета, встречный) — обе стороны сразу.
            let data = IDodoFactory::getDODOPoolBidirectionCall {
                token0: c.coin,
                token1: c.quote,
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
            None => Verdict::Failed("нет ответа реестра фабрики DODO".into()),
            Some(Err(node)) => Verdict::Failed(node),
            Some(Ok(Err(rev))) => {
                Verdict::Rejected(format!("реестр фабрики DODO откатился: {rev}"))
            }
            Some(Ok(Ok(b))) => match decide(c, pool, &b) {
                Ok(f) => Verdict::Taken(Box::new(f)),
                Err(w) => Verdict::Rejected(w),
            },
        });
    }
    out
}

/// Решение по ответу реестра (без сети). Принимает: кандидата, адрес пула, ответ getDODOPoolBidirection.
/// Возвращает: найденный пул или причину отказа. base0 — пулы с base = coin, base1 — с base = quote.
pub fn decide(c: &Candidate, pool: Address, answer: &[u8]) -> Result<Found, String> {
    let r = IDodoFactory::getDODOPoolBidirectionCall::abi_decode_returns(answer)
        .map_err(|e| format!("ответ реестра фабрики DODO не по ABI: {e}"))?;
    let in_base0 = r.baseToken0Pools.contains(&pool);
    let in_base1 = r.baseToken1Pools.contains(&pool);
    if !in_base0 && !in_base1 {
        return Err(format!(
            "фабрика DODO (dex.factory DEX {}) не знает этот пул для пары {} / {}",
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
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    })
}
