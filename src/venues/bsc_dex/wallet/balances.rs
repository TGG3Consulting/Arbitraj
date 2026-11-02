//! Файл: venues/bsc_dex/wallet/balances.rs
//! Что делает: балансы кошелька бота — BNB на газ (eth_getBalance) и все известные токены (ERC-20 balanceOf).
//! Решение 113: все балансы — одним eth_call Multicall3 (balances_mc.rs; dex.multicall3 не задан — по одному, как
//! раньше); баланс одного актива (контракт balance — предпроверка, USDT перед покупкой, перегон) — 1–2 запроса.
//! Кто вызывает: venue.rs (контракт: balances, balance), connection.rs (BNB в «Проверить подключение»), exec/* (факт).
//! От чего зависит: ctx.rs, node/rpc.rs, node/call.rs, node/abi.rs, tokens.rs, num.rs, wallet/signer.rs,
//! wallet/balances_mc.rs.
//! Настройки: узел — bsc.node_http_url; dex.multicall3, dex.load_batch (через balances_mc.rs).
//!
//! eth_getBalance(address, block) → wei —
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/state.yaml ;
//! balanceOf(owner) — https://eips.ethereum.org/EIPS/eip-20 .

use alloy_primitives::{Address, U256};
use futures_util::future::join_all;
use rust_decimal::Decimal;
use serde_json::json;

use crate::core::types::Balance;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::{call, rpc};
use super::super::num;
use super::super::tokens::{Token, Tokens};
use super::signer;

/// Как ядро называет нативную монету сети для газа (стык: core/types/cost.rs, поле Cost.asset — «BNB»).
pub const BNB: &str = "BNB";

/// Баланс BNB кошелька.
/// Принимает: адрес. Возвращает: BNB. Ошибки: узел; BadResponse — сумма не помещается в Decimal.
pub async fn bnb(ctx: &Ctx, owner: Address) -> GatewayResult<Decimal> {
    let wei: U256 = rpc::call(ctx, "eth_getBalance", json!([owner, "latest"])).await?;
    wei_to_bnb(ctx, wei)
}

/// Баланс одного токена в минимальных единицах (для проверки факта сделки).
/// Принимает: токен, владельца. Возвращает: U256. Ошибки: узел, контракт.
pub async fn token_units(ctx: &Ctx, token: Address, owner: Address) -> GatewayResult<U256> {
    call::view(ctx, token, &IERC20::balanceOfCall { owner }, "latest").await
}

/// Все балансы кошелька: BNB и каждый известный токен.
/// Принимает: список токенов. Возвращает: балансы (locked = 0: у кошелька нет заблокированных сумм; исключение —
/// обёртка нативной монеты WBNB в строке BNB, решение 62).
/// Ошибки: ключ кошелька, узел; ошибка по любому токену — ошибка целиком (половина балансов — не факт).
pub async fn all(ctx: &Ctx, tokens: &Tokens) -> GatewayResult<Vec<Balance>> {
    let owner = signer::address(ctx)?;
    let list = tokens.all();
    let addrs: Vec<Address> = list.iter().map(|t| t.address).collect();
    // Решение 113: одним eth_call Multicall3; не задан — по одному запросу на токен, как раньше.
    let (bnb_free, found) = match super::balances_mc::all_units(ctx, owner, &addrs).await? {
        Some((wei, units)) => {
            let mut out = Vec::with_capacity(list.len());
            for (t, u) in list.iter().zip(units) {
                out.push(to_coins(ctx, tokens, t, u).await?);
            }
            (wei_to_bnb(ctx, wei)?, out)
        }
        None => {
            let bnb_free = bnb(ctx, owner).await?;
            let mut out = Vec::with_capacity(list.len());
            for r in join_all(list.iter().map(|t| one(ctx, tokens, t, owner))).await {
                out.push(r?);
            }
            (bnb_free, out)
        }
    };
    let mut out = vec![balance(ctx, BNB, bnb_free)];
    let native = super::super::exec::native::wrapped(ctx);
    for (t, free) in list.iter().zip(found) {
        // Нативная монета (решение 62): WBNB — та же монета BNB, но на газ и нативный перевод не годится без
        // UNWRAP — в locked строки BNB (сверка ядра считает free + locked, газ — только free).
        if is_native_row(&native, t) {
            out[0].locked += free;
            continue;
        }
        out.push(balance(ctx, &t.ticker, free));
    }
    Ok(out)
}

/// Баланс одного актива (решение 113) — та же строка, что дал бы all(), но 1–2 запроса вместо запроса по каждому
/// токену. BNB: eth_getBalance (free) и, если WBNB нативной монеты в списке токенов, его balanceOf (locked, решение 62).
/// Токен: его balanceOf. Принимает: тикер. Возвращает: None — актива нет среди известных токенов.
/// Ошибки: ключ кошелька, узел, контракт.
pub async fn single(ctx: &Ctx, tokens: &Tokens, asset: &str) -> GatewayResult<Option<Balance>> {
    let owner = signer::address(ctx)?;
    let list = tokens.all();
    if asset == BNB {
        let native = super::super::exec::native::wrapped(ctx);
        let mut row = balance(ctx, BNB, bnb(ctx, owner).await?);
        for t in list.iter().filter(|t| is_native_row(&native, t)) {
            row.locked += one(ctx, tokens, t, owner).await?;
        }
        return Ok(Some(row));
    }
    match list.iter().find(|t| t.ticker == asset) {
        Some(t) => Ok(Some(balance(
            ctx,
            &t.ticker,
            one(ctx, tokens, t, owner).await?,
        ))),
        None => Ok(None),
    }
}

/// Токен — обёртка нативной монеты (WBNB под тикером BNB, решение 62): его баланс идёт в locked строки BNB. Ошибок нет.
fn is_native_row(native: &Option<(String, Address)>, t: &Token) -> bool {
    native.as_ref() == Some(&(t.ticker.clone(), t.address)) && t.ticker == BNB
}

/// wei → BNB. Ошибки: BadResponse — не помещается в Decimal.
fn wei_to_bnb(ctx: &Ctx, wei: U256) -> GatewayResult<Decimal> {
    num::to_decimal(wei, num::BNB_DECIMALS).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Баланс BNB {wei} wei не помещается в Decimal"),
        )
    })
}

/// Минимальные единицы токена → монеты (decimals из кэша или контракта). Ошибки: узел, не помещается в Decimal.
async fn to_coins(ctx: &Ctx, tokens: &Tokens, t: &Token, units: U256) -> GatewayResult<Decimal> {
    let d = tokens.decimals(ctx, t.address).await?;
    num::to_decimal(units, d).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Баланс {} ({units}) не помещается в Decimal", t.ticker),
        )
    })
}

/// Баланс одного токена в монетах. Ошибки: узел, контракт, не помещается в Decimal.
async fn one(ctx: &Ctx, tokens: &Tokens, t: &Token, owner: Address) -> GatewayResult<Decimal> {
    let units = token_units(ctx, t.address, owner).await?;
    to_coins(ctx, tokens, t, units).await
}

/// Строка баланса в формате ядра.
fn balance(ctx: &Ctx, asset: &str, free: Decimal) -> Balance {
    Balance {
        venue: ctx.venue(),
        asset: asset.to_string(),
        free,
        locked: Decimal::ZERO,
    }
}
