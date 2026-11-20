//! Файл: venues/bsc_dex/pools/verify_wombat.rs
//! Что делает: решение 484 — проверка пула Wombat Exchange из справочника или списка (решение 59) на своём узле: пул —
//! в dex.stable_pools DEX (фабрики у Wombat нет, адреса пулов — официальная документация docs.wombat.exchange
//! /developers/contracts/arbitrum), обе монеты пары — среди getTokens() пула (CLAUDE.md: адрес токена у биржи = в пуле),
//! реализация прокси (слот EIP-1967) — в dex.wombat_implementations или dex.wombat_dynamic_implementations (код сверен с
//! исходником вида; load_wombat::kind_of). Решение 295 — multi_pair::adopt (вид многомонетный).
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs, node/rpc.rs, abi_wombat.rs, load_wombat.rs (kind_of), verify.rs (Candidate,
//! Verdict), verify_woo.rs (found), multi_pair.rs.
//! Настройки: dex.stable_pools, dex.wombat_implementations, dex.wombat_dynamic_implementations [DEX вида wombat].

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes};
use serde_json::json;

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::{call, rpc};
use super::abi_wombat::{IWombatPool as P, implementation_slot};
use super::load_wombat::kind_of;
use super::load_wombat_memo;
use super::multi_pair;
use super::verify::{Candidate, Verdict};
use super::verify_woo;

/// Проверка пачки кандидатов вида wombat. Возвращает: итог по каждому. Ошибок нет (отказ — с причиной; узел — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate]) -> Vec<Verdict> {
    let mut out = Vec::with_capacity(cands.len());
    for c in cands {
        out.push(match one(ctx, c).await {
            Ok(Ok(f)) => Verdict::Taken(Box::new(f)),
            Ok(Err(why)) => Verdict::Rejected(why),
            Err(node) => Verdict::Failed(node),
        });
    }
    out
}

/// Пул в dex.stable_pools DEX? Ошибок нет (не задана — нет).
fn listed(ctx: &Ctx, c: &Candidate, pool: Address) -> bool {
    ctx.dex_list("dex.stable_pools", c.family.id)
        .map(|l| l.iter().any(|s| Address::from_str(s.trim()).ok() == Some(pool)))
        .unwrap_or(false)
}

/// Один кандидат. Возвращает: Ok(Ok(пул)) — принят, Ok(Err) — отказ с причиной, Err — сбой узла (текст).
async fn one(ctx: &Ctx, c: &Candidate) -> Result<Result<super::discover::Found, String>, String> {
    let Ok(pool) = Address::from_str(&c.pool) else {
        return Ok(Err(format!("«{}» — не адрес контракта пула", c.pool)));
    };
    if !listed(ctx, c, pool) {
        return Ok(Err(format!(
            "пул {pool:#x} не подтверждён: нет в dex.stable_pools DEX {}",
            c.family.id
        )));
    }
    let calls: Vec<(Address, Bytes)> = vec![(pool, P::getTokensCall {}.abi_encode().into())];
    let got = call::try_many(ctx, &calls, "latest").await.map_err(|e| e.message)?;
    let tokens = match got.first() {
        Some(Ok(b)) => match P::getTokensCall::abi_decode_returns(b) {
            Ok(t) => t,
            Err(e) => return Ok(Err(format!("getTokens() пула {pool:#x} не по ABI: {e}"))),
        },
        Some(Err(r)) => return Ok(Err(format!("getTokens() пула {pool:#x} откатился: {r}"))),
        None => return Err("нет ответа getTokens()".into()),
    };
    if !tokens.contains(&c.coin) || !tokens.contains(&c.quote) {
        return Ok(Err(format!(
            "среди монет пула {pool:#x} нет пары {} / {}",
            c.coin, c.quote
        )));
    }
    let word: B256 = rpc::call(
        ctx,
        "eth_getStorageAt",
        json!([pool, implementation_slot(), "latest"]),
    )
    .await
    .map_err(|e| e.message)?;
    if let Err(why) = kind_of(ctx, c.family, Address::from_word(word)) {
        return Ok(Err(format!("пул {pool:#x}: {why}")));
    }
    // Ревью t253: проверка — полное чтение пула заново (монеты и активы из памяти забываются).
    load_wombat_memo::forget(c.family.id, pool);
    let found = verify_woo::found(c, PoolRef::Contract(pool));
    Ok(Ok(multi_pair::adopt(ctx, found, &tokens)))
}
