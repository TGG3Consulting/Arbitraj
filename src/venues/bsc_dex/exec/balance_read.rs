//! Файл: venues/bsc_dex/exec/balance_read.rs
//! Что делает: решение 222 — чтение блока свопа для факта по балансу (exec/balance_fact.rs) одной пачкой JSON-RPC:
//! события Transfer монеты с кошелька и на кошелёк во всём блоке (eth_getLogs), balanceOf кошелька на блок−1 и на блок.
//! Кто вызывает: exec/balance_fact.rs.
//! От чего зависит: node/rpc.rs (пачка), node/call.rs, node/jsonrpc.rs, node/abi.rs.
//! Настройки: узел — bsc.node_http_url (через node/rpc.rs).
//!
//! balanceOf(owner) — https://eips.ethereum.org/EIPS/eip-20#balanceof ; Transfer(from indexed, to indexed, value) —
//! https://eips.ethereum.org/EIPS/eip-20#transfer-1 ; eth_call(transaction, blockNumber), eth_getLogs(filter {fromBlock,
//! toBlock, address, topics}) — https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth .

use alloy::rpc::types::Log;
use alloy::sol_types::{SolCall, SolEvent};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::value::RawValue;
use serde_json::{Value, json};

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::{call, jsonrpc, rpc};
use super::balance_fact::Seen;

/// Блок свопа: Transfer монеты с кошелька / на кошелёк в блоке, balanceOf на блок−1 и на блок. Принимает: токен,
/// кошелёк, номер блока. Возвращает: Seen. Ошибки — текст (узел, нет состояния блока−1, разбор ответа).
pub async fn read(ctx: &Ctx, token: Address, wallet: Address, block: u64) -> Result<Seen, String> {
    let prev = block.checked_sub(1).ok_or("блок 0")?;
    let (t0, w) = (
        IERC20::Transfer::SIGNATURE_HASH,
        B256::left_padding_from(wallet.as_slice()),
    );
    let tag = call::block_tag(block);
    let logs = |topics: Value| json!([{ "fromBlock": tag, "toBlock": tag, "address": token, "topics": topics }]);
    let data = Bytes::from(IERC20::balanceOfCall { owner: wallet }.abi_encode());
    let bal = |b: u64| json!([{ "to": token, "input": data }, call::block_tag(b)]);
    let calls = [
        ("eth_getLogs", logs(json!([t0, w]))),
        ("eth_getLogs", logs(json!([t0, Value::Null, w]))),
        ("eth_call", bal(prev)),
        ("eth_call", bal(block)),
    ];
    let mut out = rpc::batch(ctx, &calls)
        .await
        .map_err(|e| e.message)?
        .into_iter();
    let mut next = || out.next().ok_or("неполный ответ пачки".to_string());
    let src = rpc::SOURCE;
    let sum = |raw: Box<RawValue>| -> Result<U256, String> {
        let list: Vec<Log> = jsonrpc::decode(&raw, src).map_err(|e| e.message)?;
        Ok(transfers_sum(&list))
    };
    let block_out = sum(next()?.map_err(|e| e.message)?)?;
    let block_in = sum(next()?.map_err(|e| e.message)?)?;
    let balance = |raw: Box<RawValue>| -> Result<U256, String> {
        let b: Bytes = jsonrpc::decode(&raw, src).map_err(|e| e.message)?;
        call::decode::<IERC20::balanceOfCall>(&b).map_err(|e| e.message)
    };
    let before = balance(next()?.map_err(|e| format!("баланс на блок {prev}: {}", e.message))?)?;
    let after = balance(next()?.map_err(|e| format!("баланс на блок {block}: {}", e.message))?)?;
    Ok(Seen {
        before,
        after,
        block_out,
        block_in,
    })
}

/// Сумма value событий Transfer (ERC-20: три topics; у ERC-721 четыре — пропускаются). Ошибок нет.
pub fn transfers_sum(logs: &[Log]) -> U256 {
    logs.iter()
        .filter(|l| l.topics().len() == 3)
        .filter_map(|l| IERC20::Transfer::decode_log_data(&l.inner.data).ok())
        .fold(U256::ZERO, |acc, t| acc.saturating_add(t.value))
}
