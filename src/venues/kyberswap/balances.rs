//! Файл: venues/kyberswap/balances.rs
//! Что делает: балансы кошелька бота — BNB на газ (eth_getBalance) и известные токены (ERC-20 balanceOf), а также
//! decimals токена из контракта с кэшем. Своя копия адаптера kyberswap (изоляция), на alloy/узел напрямую.
//! Кто вызывает: venue.rs (balances), connection.rs, quote.rs, trade.rs, transfer.rs.
//! От чего зависит: ctx.rs, node.rs, abi.rs, num.rs, signer.rs, state.rs (кэш decimals и список монет).
//! Настройки: узел — kyber.node_http_url.
//!
//! eth_getBalance — execution-apis src/eth/state.yaml; balanceOf, decimals — EIP-20.

use alloy_primitives::{Address, U256};
use futures_util::future::join_all;
use rust_decimal::Decimal;

use crate::core::types::Balance;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::abi::IERC20;
use super::ctx::Ctx;
use super::state::Parts;
use super::{node, num, signer};

/// Как ядро называет нативную монету сети для газа (стык core/types/cost.rs — «BNB»).
pub const BNB: &str = "BNB";

/// Баланс BNB кошелька. Принимает: адрес. Возвращает: BNB. Ошибки: узел; BadResponse — не помещается в Decimal.
pub async fn bnb(ctx: &Ctx, owner: Address) -> GatewayResult<Decimal> {
    let wei = node::balance(ctx, owner).await?;
    num::to_decimal(wei, num::BNB_DECIMALS).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Баланс BNB {wei} wei не помещается в Decimal"),
        )
    })
}

/// Баланс токена в минимальных единицах (для факта сделки). Ошибки: узел, контракт.
pub async fn token_units(ctx: &Ctx, token: Address, owner: Address) -> GatewayResult<U256> {
    node::view(ctx, token, &IERC20::balanceOfCall { owner }).await
}

/// decimals токена: из кэша или из контракта (ERC-20 decimals()). Ошибки: узел, контракт не ERC-20.
pub async fn decimals(p: &Parts, token: Address) -> GatewayResult<u8> {
    if let Some(d) = p.decimals_known(token) {
        return Ok(d);
    }
    let d: u8 = node::view(&p.ctx, token, &IERC20::decimalsCall {}).await?;
    p.remember_decimals(token, d);
    Ok(d)
}

/// Все балансы кошелька: BNB и каждая известная монета подписки с адресом контракта.
/// Ошибки: ключ кошелька, узел; ошибка по любому токену — ошибка целиком (половина балансов — не факт).
pub async fn all(p: &Parts) -> GatewayResult<Vec<Balance>> {
    let ctx = &p.ctx;
    let owner = signer::address(ctx)?;
    let mut out = vec![row(ctx, BNB, bnb(ctx, owner).await?)];
    let tokens: Vec<(String, Address)> = p
        .assets()
        .into_iter()
        .filter_map(|a| a.bsc_contract.map(|addr| (a.ticker, addr)))
        .collect();
    let found = join_all(tokens.iter().map(|(_, a)| one(p, *a, owner))).await;
    for ((ticker, _), r) in tokens.iter().zip(found) {
        out.push(row(ctx, ticker, r?));
    }
    Ok(out)
}

/// Баланс одного токена в монетах. Ошибки: узел, контракт, не помещается в Decimal.
async fn one(p: &Parts, token: Address, owner: Address) -> GatewayResult<Decimal> {
    let d = decimals(p, token).await?;
    let units = token_units(&p.ctx, token, owner).await?;
    num::to_decimal(units, d).ok_or_else(|| {
        p.ctx.err(
            ErrorCode::BadResponse,
            format!("Баланс токена {units} (decimals {d}) не помещается в Decimal"),
        )
    })
}

/// Строка баланса в формате ядра (у кошелька нет заблокированных сумм). Ошибок нет.
fn row(ctx: &Ctx, asset: &str, free: Decimal) -> Balance {
    Balance {
        venue: ctx.venue(),
        asset: asset.to_string(),
        free,
        locked: Decimal::ZERO,
    }
}
