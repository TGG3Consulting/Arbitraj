//! Файл: venues/bsc_dex/node/call.rs
//! Что делает: eth_call к контракту с ABI-кодированием запроса и разбором ответа (alloy sol!); пачка вызовов —
//! через Multicall3 (dex.multicall3) или пачкой JSON-RPC.
//! Кто вызывает: wallet/*, pools/*, check/*, quote/*, exec/*.
//! От чего зависит: node/rpc.rs, errors.rs, alloy::sol_types.
//! Настройки: узел — bsc.node_http_url.
//!
//! eth_call(transaction, block) → bytes — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/execute.yaml
//! ABI-кодирование — https://docs.soliditylang.org/en/latest/abi-spec.html

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};
use serde_json::json;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::errors;
use super::rpc::{self, SOURCE};

/// Вызов view-функции контракта.
/// Принимает: адрес контракта, вызов (тип sol!), блок («latest» или номер в hex).
/// Возвращает: разобранный ответ функции.
/// Ошибки: ответ-ошибка узла (откат — SwapReverted с текстом узла), BadResponse (не тот формат ответа).
pub async fn view<C: SolCall>(
    ctx: &Ctx,
    to: Address,
    call: &C,
    block: &str,
) -> GatewayResult<C::Return> {
    let data = Bytes::from(call.abi_encode());
    let out: Bytes =
        rpc::call(ctx, "eth_call", json!([{ "to": to, "input": data }, block])).await?;
    decode::<C>(&out)
}

/// Несколько вызовов одного вида на одном блоке: при заданном dex.multicall3 — одним eth_call через Multicall3
/// (node/multicall.rs), иначе пачкой JSON-RPC одним HTTP-запросом. Принимает: адрес контракта, вызовы, блок.
/// Возвращает: ответы в том же порядке. Ошибки: любая ошибка любого вызова — ошибка целиком (половина данных — не факт).
pub async fn view_many<C: SolCall>(
    ctx: &Ctx,
    to: Address,
    calls: &[C],
    block: &str,
) -> GatewayResult<Vec<C::Return>> {
    if calls.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(calls.len());
    if let Some(mc) = super::multicall::address(ctx) {
        let list: Vec<(Address, Bytes)> = calls
            .iter()
            .map(|c| (to, Bytes::from(c.abi_encode())))
            .collect();
        for (i, r) in super::multicall::aggregate(ctx, mc, &list, block)
            .await?
            .into_iter()
            .enumerate()
        {
            let bytes = r.map_err(|why| {
                errors::bad(
                    &format!("{} №{i} через Multicall3: {why}", C::SIGNATURE),
                    SOURCE,
                    "",
                )
            })?;
            out.push(decode::<C>(&bytes)?);
        }
        return Ok(out);
    }
    let params: Vec<(&str, serde_json::Value)> = calls
        .iter()
        .map(|c| {
            let data = Bytes::from(c.abi_encode());
            ("eth_call", json!([{ "to": to, "input": data }, block]))
        })
        .collect();
    for r in rpc::batch(ctx, &params).await? {
        let bytes: Bytes = super::jsonrpc::decode(&r?, SOURCE)?;
        out.push(decode::<C>(&bytes)?);
    }
    Ok(out)
}

/// Много вызовов одного вида на одном блоке (поиск пулов по монетам): при заданном dex.multicall3 — одним eth_call
/// (view_many), иначе — отдельными eth_call, как раньше (большую пачку JSON-RPC провайдер может не принять).
/// Принимает: адрес контракта, вызовы, блок. Возвращает: ответы в том же порядке. Ошибки: как у view_many / view.
pub async fn view_list<C: SolCall>(
    ctx: &Ctx,
    to: Address,
    calls: &[C],
    block: &str,
) -> GatewayResult<Vec<C::Return>> {
    if super::multicall::address(ctx).is_some() {
        return view_many(ctx, to, calls, block).await;
    }
    let each = calls.iter().map(|c| view(ctx, to, c, block));
    futures_util::future::join_all(each)
        .await
        .into_iter()
        .collect()
}

/// Разные вызовы (разные контракты и функции) на одном блоке, каждый со своим итогом: при заданном dex.multicall3 —
/// одним eth_call через Multicall3.aggregate3 (allowFailure), иначе пачкой JSON-RPC. Нужен проверке пулов справочника:
/// поддельный «пул» может откатить вызов — это ответ, а не сбой. Принимает: (контракт, данные), блок.
/// Возвращает: по каждому вызову байты ответа или текст отката. Ошибки: узел, BadResponse (ответ aggregate3).
pub async fn try_many(
    ctx: &Ctx,
    calls: &[(Address, Bytes)],
    block: &str,
) -> GatewayResult<Vec<Result<Bytes, String>>> {
    if calls.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(mc) = super::multicall::address(ctx) {
        return super::multicall::aggregate(ctx, mc, calls, block).await;
    }
    let params: Vec<(&str, serde_json::Value)> = calls
        .iter()
        .map(|(to, data)| ("eth_call", json!([{ "to": to, "input": data }, block])))
        .collect();
    let mut out = Vec::with_capacity(calls.len());
    for r in rpc::batch(ctx, &params).await? {
        out.push(match r {
            Ok(v) => super::jsonrpc::decode::<Bytes>(&v, SOURCE).map_err(|e| e.message),
            Err(e) => Err(e.message),
        });
    }
    Ok(out)
}

/// Разбор ответа функции из байт.
/// Принимает: байты ответа. Возвращает: значение. Ошибка BadResponse — байты не по ABI функции.
pub fn decode<C: SolCall>(out: &[u8]) -> GatewayResult<C::Return> {
    C::abi_decode_returns(out).map_err(|e| {
        errors::bad(
            &format!("ответ {} не по ABI: {e}", C::SIGNATURE),
            SOURCE,
            &alloy_primitives::hex::encode_prefixed(out),
        )
    })
}

/// Номер блока для параметра запроса: 0x… (execution-apis, BlockNumberOrTag). Ошибок нет.
pub fn block_tag(number: u64) -> String {
    format!("{number:#x}")
}
