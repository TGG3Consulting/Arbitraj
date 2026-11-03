//! Файл: venues/bsc_dex/pools/verify_woo.rs
//! Что делает: решение по паре WooFi WooPPV2 (решение 122 F, 3) по ответам quoteToken / wooracle / state(монета)
//! оракула и слоту реализации прокси EIP-1967: пара принимается, если котируемая монета = встречный токен, оракул пула
//! = dex.oracle, реализация = dex.pool_implementation и оракул отдаёт цену монеты; ссылка — PoolRef::Member. Здесь же —
//! сборка найденного пула по кандидату (общая с verify_stable_u.rs).
//! Кто вызывает: pools/verify_stable_u.rs.
//! От чего зависит: ctx.rs, node/rpc.rs, abi_woo.rs, discover.rs, verify.rs.
//! Настройки: dex.oracle, dex.pool_implementation [DEX].

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes};
use serde_json::json;

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::abi_woo::{IWooPP, IWooracle, implementation_slot};
use super::discover::{Found, sorted};
use super::verify::Candidate;

/// WooFi: решение по ответам quoteToken / wooracle / state и слоту реализации. Err — сбой узла.
pub async fn woo(
    ctx: &Ctx,
    c: &Candidate,
    pool: Address,
    got: &[Result<Bytes, String>],
) -> Result<Result<Found, String>, String> {
    let id = c.family.id;
    let dec = |i: usize| got.get(i).and_then(|r| r.as_ref().ok());
    let quote = dec(0).and_then(|b| IWooPP::quoteTokenCall::abi_decode_returns(b).ok());
    if quote != Some(c.quote) {
        return Ok(Err(format!(
            "котируемая монета WooPPV2 {quote:?} ≠ встречный токен {}",
            c.quote
        )));
    }
    let oracle = ctx.dex_addr("dex.oracle", id).map_err(|e| e.message)?;
    let have = dec(1).and_then(|b| IWooPP::wooracleCall::abi_decode_returns(b).ok());
    if have != Some(oracle) {
        return Ok(Err(format!(
            "оракул WooPPV2 {have:?} ≠ dex.oracle {oracle:#x}"
        )));
    }
    let price = dec(2)
        .and_then(|b| IWooracle::stateCall::abi_decode_returns(b).ok())
        .map(|s| s.price);
    if price.unwrap_or(0) == 0 {
        return Ok(Err(format!(
            "оракул WooFi не отдаёт цену монеты {} (не поддержана или пауза)",
            c.coin
        )));
    }
    let word: B256 = rpc::call(
        ctx,
        "eth_getStorageAt",
        json!([pool, implementation_slot(), "latest"]),
    )
    .await
    .map_err(|e| e.message)?;
    let pinned = ctx
        .dex_addr("dex.pool_implementation", id)
        .map_err(|e| e.message)?;
    let imp = Address::from_word(word);
    if imp != pinned {
        return Ok(Err(format!(
            "реализация прокси WooPPV2 {imp:#x} ≠ dex.pool_implementation {pinned:#x} — контракт обновили, сверить"
        )));
    }
    Ok(Ok(found(
        c,
        PoolRef::Member {
            contract: pool,
            coin: c.coin,
        },
    )))
}

/// Найденный пул по кандидату и ссылке. Ошибок нет.
pub fn found(c: &Candidate, reference: PoolRef) -> Found {
    let (token0, token1) = sorted(c.coin, c.quote);
    Found {
        family: c.family,
        reference,
        token0,
        token1,
        coin: c.coin,
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    }
}
