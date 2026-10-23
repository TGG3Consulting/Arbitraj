//! Файл: venues/bsc_dex/wallet/balances.rs
//! Что делает: балансы кошелька бота — BNB на газ (eth_getBalance) и все известные токены (ERC-20 balanceOf).
//! Кто вызывает: mod.rs (контракт: balances), connection.rs (BNB в «Проверить подключение»), exec/* (факт).
//! От чего зависит: ctx.rs, node/rpc.rs, node/call.rs, node/abi.rs, tokens.rs, num.rs, wallet/signer.rs.
//! Настройки: узел — bsc.node_http_url.
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
    num::to_decimal(wei, num::BNB_DECIMALS).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Баланс BNB {wei} wei не помещается в Decimal"),
        )
    })
}

/// Баланс одного токена в минимальных единицах (для проверки факта сделки).
/// Принимает: токен, владельца. Возвращает: U256. Ошибки: узел, контракт.
pub async fn token_units(ctx: &Ctx, token: Address, owner: Address) -> GatewayResult<U256> {
    call::view(ctx, token, &IERC20::balanceOfCall { owner }, "latest").await
}

/// Все балансы кошелька: BNB и каждый известный токен.
/// Принимает: список токенов. Возвращает: балансы (locked = 0: у кошелька нет заблокированных сумм).
/// Ошибки: ключ кошелька, узел; ошибка по любому токену — ошибка целиком (половина балансов — не факт).
pub async fn all(ctx: &Ctx, tokens: &Tokens) -> GatewayResult<Vec<Balance>> {
    let owner = signer::address(ctx)?;
    let mut out = vec![balance(ctx, BNB, bnb(ctx, owner).await?)];
    let list = tokens.all();
    let found = join_all(list.iter().map(|t| one(ctx, tokens, t, owner))).await;
    for (t, r) in list.iter().zip(found) {
        out.push(balance(ctx, &t.ticker, r?));
    }
    Ok(out)
}

/// Баланс одного токена в монетах. Ошибки: узел, контракт, не помещается в Decimal.
async fn one(ctx: &Ctx, tokens: &Tokens, t: &Token, owner: Address) -> GatewayResult<Decimal> {
    let d = tokens.decimals(ctx, t.address).await?;
    let units = token_units(ctx, t.address, owner).await?;
    num::to_decimal(units, d).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Баланс {} ({units}) не помещается в Decimal", t.ticker),
        )
    })
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
