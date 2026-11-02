//! Файл: venues/bsc_dex/wallet/balances_mc.rs
//! Что делает: все балансы кошелька одним eth_call через Multicall3 (решение 113). Раньше balances.rs::all спрашивал
//! узел отдельно про каждый известный токен (~190 eth_call + eth_getBalance) — при темпе bsc.node_http_rps = 20 это
//! ~11 с на один вызов balances(). Здесь BNB (Multicall3.getEthBalance) и balanceOf всех токенов — в одном aggregate3,
//! на одном блоке; при dex.load_batch — частями не больше dex.load_batch вызовов. Решение 121 (A6): частей больше
//! одной — все на одном номере блока (голова eth_blockNumber на момент запроса), а не «latest» каждой части: иначе
//! части читают разные блоки и балансы между собой не сходятся.
//! Проверено 02.11.2026 на живом кошельке бота через bsc-rpc.publicnode.com: 190 вызовов одним eth_call — 0.4 с,
//! 190 из 190 успешны, BNB и USDT совпали с отдельными eth_getBalance и balanceOf.
//! Кто вызывает: wallet/balances.rs (all).
//! От чего зависит: node/multicall.rs, node/call.rs (decode, block_tag), node/rpc.rs (eth_blockNumber), node/abi.rs (IERC20).
//! Настройки: dex.multicall3 [bsc_dex] (не задан — None, balances.rs спрашивает по одному, как раньше),
//! dex.load_batch [bsc_dex] (не задана — одна часть).
//!
//! Multicall3: aggregate3 и getEthBalance(address) — https://github.com/mds1/multicall3 (src/Multicall3.sol);
//! balanceOf(owner) — https://eips.ethereum.org/EIPS/eip-20 ; eth_blockNumber, eth_call с номером блока
//! (BlockNumberOrTag) — https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth .

// Код вызовов генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U64, U256};
use serde_json::json;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::errors;
use super::super::node::abi::IERC20;
use super::super::node::rpc::{self, SOURCE};
use super::super::node::{call, multicall};

alloy::sol! {
    interface IMulticall3Eth {
        function getEthBalance(address addr) external view returns (uint256 balance);
    }
}

/// BNB (wei) и балансы токенов (в минимальных единицах, в порядке `tokens`) одним eth_call Multicall3 на «latest»;
/// частями (dex.load_batch) — все части на одном номере блока (eth_blockNumber перед первой частью).
/// Принимает: окружение, владельца, адреса токенов. Возвращает: None — dex.multicall3 не задан.
/// Ошибки: узел; BadResponse — ответ не по ABI или откат любого вызова (половина балансов — не факт); StaleData —
/// состояния блока у узла уже нет (errors.rs).
pub async fn all_units(
    ctx: &Ctx,
    owner: Address,
    tokens: &[Address],
) -> GatewayResult<Option<(U256, Vec<U256>)>> {
    let Some(mc) = multicall::address(ctx) else {
        return Ok(None);
    };
    let mut calls: Vec<(Address, Bytes)> = Vec::with_capacity(tokens.len() + 1);
    calls.push((
        mc,
        Bytes::from(IMulticall3Eth::getEthBalanceCall { addr: owner }.abi_encode()),
    ));
    let of = Bytes::from(IERC20::balanceOfCall { owner }.abi_encode());
    calls.extend(tokens.iter().map(|t| (*t, of.clone())));
    let part = match ctx.int("dex.load_batch") {
        Ok(n) if n > 0 => n as usize,
        _ => calls.len(),
    };
    // Одна часть — «latest» (один eth_call — один блок). Несколько — номер головы, общий для всех частей.
    let block = if calls.len() > part {
        let n: U64 = rpc::call(ctx, "eth_blockNumber", json!([])).await?;
        call::block_tag(n.to())
    } else {
        "latest".to_string()
    };
    let mut raw = Vec::with_capacity(calls.len());
    for chunk in calls.chunks(part) {
        raw.extend(multicall::aggregate(ctx, mc, chunk, &block).await?);
    }
    let mut out = Vec::with_capacity(raw.len());
    for (i, r) in raw.into_iter().enumerate() {
        let bytes = r.map_err(|why| {
            let what = match i {
                0 => "getEthBalance".to_string(),
                _ => format!("balanceOf токена {}", tokens[i - 1]),
            };
            errors::bad(&format!("{what} через Multicall3: {why}"), SOURCE, "")
        })?;
        out.push(match i {
            0 => call::decode::<IMulticall3Eth::getEthBalanceCall>(&bytes)?,
            _ => call::decode::<IERC20::balanceOfCall>(&bytes)?,
        });
    }
    let bnb = out.remove(0);
    Ok(Some((bnb, out)))
}
