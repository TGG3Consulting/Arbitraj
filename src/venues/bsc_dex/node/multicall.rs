//! Файл: venues/bsc_dex/node/multicall.rs
//! Что делает: несколько view-вызовов одним eth_call через Multicall3.aggregate3 (allowFailure = true — ответ
//! каждого вызова отдельно). У NodeReal eth_call — 20 CU за вызов, а пачка JSON-RPC из N eth_call — N вызовов
//! (node/pace.rs считает так же); через Multicall3 — один eth_call на всю пачку.
//! Кто вызывает: node/call.rs (view_many, try_many), wallet/balances_mc.rs, connection_extra.rs (getBlockNumber).
//! Решение 325: aggregate сначала одной проверкой «книга есть» (node/prefetch.rs active) — книги нет (вся система, кроме
//! кода чтения пула при dex.event_refresh_batch) — сразу aggregate_raw, без выделений и поиска по вызовам (ревью 325,
//! Н1); книга есть — ответы из неё, к узлу только остальные (merge — по порядку). Само чтение заранее
//! (pools/prefetch_read.rs) идёт через try_chunks → try_many → aggregate ВНЕ книги (книга ставится только на код чтения).
//! От чего зависит: node/rpc.rs, errors.rs, alloy::sol!.
//! Настройки: dex.multicall3 [bsc_dex] — адрес Multicall3. Не задан — view_many идёт пачкой JSON-RPC, как раньше.
//!
//! Multicall3, aggregate3, структуры Call3 и Result — https://github.com/mds1/multicall3/blob/main/src/Multicall3.sol#L19-L35
//! (aggregate3 — #L98; getBlockNumber — #L170); адрес развёртывания в сетях (в том числе BSC) — README того же репозитория
//! («deployed on over 250 chains at 0xcA11bde05977b3631167028862bE2a173976CA11») — задаётся настройкой.
//! eth_call — execution-apis src/eth/execute.yaml; цены методов — https://docs.nodereal.io/docs/compute-units-cus .

// Код вызовов генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};
use serde_json::json;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::errors;
use super::rpc::{self, SOURCE};

alloy::sol! {
    interface IMulticall3 {
        struct Call3 {
            address target;
            bool allowFailure;
            bytes callData;
        }
        struct Result {
            bool success;
            bytes returnData;
        }
        function aggregate3(Call3[] calldata calls) external payable returns (Result[] memory returnData);
        // Multicall3.sol, getBlockNumber() — «Проверить подключение» (connection_extra.rs): контракт отвечает на eth_call.
        function getBlockNumber() external view returns (uint256 blockNumber);
    }
}

/// Адрес Multicall3 из настроек. None — не задан (тогда чтение пачкой JSON-RPC). Ошибок нет.
pub fn address(ctx: &Ctx) -> Option<Address> {
    ctx.addr("dex.multicall3").ok()
}

/// Кодирует пачку вызовов в один вызов aggregate3. Принимает: (контракт, данные вызова). Возвращает: данные eth_call.
/// Ошибок нет.
pub fn encode(calls: &[(Address, Bytes)]) -> Bytes {
    let list = calls
        .iter()
        .map(|(to, data)| IMulticall3::Call3 {
            target: *to,
            allowFailure: true,
            callData: data.clone(),
        })
        .collect();
    Bytes::from(IMulticall3::aggregate3Call { calls: list }.abi_encode())
}

/// Разбирает ответ aggregate3: по каждому вызову — байты ответа или текст отката (байты отката в hex).
/// Ошибки: BadResponse — ответ не по ABI или число ответов не совпадает с числом вызовов.
pub fn decode(out: &[u8], n: usize) -> GatewayResult<Vec<Result<Bytes, String>>> {
    let raw = alloy_primitives::hex::encode_prefixed(out);
    let list = IMulticall3::aggregate3Call::abi_decode_returns(out)
        .map_err(|e| errors::bad(&format!("ответ aggregate3 не по ABI: {e}"), SOURCE, &raw))?;
    if list.len() != n {
        let what = format!("aggregate3: ответов {} на {n} вызовов", list.len());
        return Err(errors::bad(&what, SOURCE, &raw));
    }
    Ok(list
        .into_iter()
        .map(|r| match r.success {
            true => Ok(r.returnData),
            false => Err(format!(
                "вызов откатился: {}",
                alloy_primitives::hex::encode_prefixed(&r.returnData)
            )),
        })
        .collect())
}

/// Пачка вызовов одним eth_call к Multicall3 на блоке. Решение 325: вызовы, ответ которых уже есть в книге задачи
/// (node/prefetch.rs, тот же блок), берутся из неё, к узлу идут только остальные (одним eth_call); книги нет или в ней
/// ничего — как раньше. Принимает: адрес Multicall3, вызовы, блок.
/// Возвращает: ответ каждого вызова отдельно, по порядку вызовов. Ошибки: узел, BadResponse.
pub async fn aggregate(
    ctx: &Ctx,
    multicall: Address,
    calls: &[(Address, Bytes)],
    block: &str,
) -> GatewayResult<Vec<Result<Bytes, String>>> {
    // Ревью 325 (Н1): книги нет — одна проверка task_local, дальше как до решения 325.
    if !super::prefetch::active() {
        return aggregate_raw(ctx, multicall, calls, block).await;
    }
    let known: Vec<Option<Bytes>> = calls
        .iter()
        .map(|(to, data)| super::prefetch::hit(*to, data, block))
        .collect();
    if known.iter().all(Option::is_none) {
        return aggregate_raw(ctx, multicall, calls, block).await;
    }
    let rest: Vec<(Address, Bytes)> = calls
        .iter()
        .zip(&known)
        .filter(|(_, k)| k.is_none())
        .map(|(c, _)| c.clone())
        .collect();
    let got = if rest.is_empty() {
        Vec::new()
    } else {
        aggregate_raw(ctx, multicall, &rest, block).await?
    };
    Ok(merge(known, got))
}

/// Решение 325: ответы по порядку вызовов — из книги (Some) и от узла (по порядку тех, кого в книге нет).
/// Принимает: ответы книги по каждому вызову, ответы узла на вызовы без ответа книги. Возвращает: ответ каждого
/// вызова. Ошибок нет: ответов узла меньше — у недостающих Err (decode уже проверил, что их столько же).
pub fn merge(
    known: Vec<Option<Bytes>>,
    got: Vec<Result<Bytes, String>>,
) -> Vec<Result<Bytes, String>> {
    let mut got = got.into_iter();
    known
        .into_iter()
        .map(|k| match k {
            Some(b) => Ok(b),
            None => got
                .next()
                .unwrap_or_else(|| Err("aggregate3: ответов меньше, чем вызовов".to_string())),
        })
        .collect()
}

/// Пачка вызовов одним eth_call к Multicall3 на блоке, без книги заранее прочитанных ответов (книги нет — aggregate
/// идёт сюда сразу). Принимает: адрес Multicall3, вызовы, блок.
/// Возвращает: ответ каждого вызова отдельно. Ошибки: узел, BadResponse.
pub async fn aggregate_raw(
    ctx: &Ctx,
    multicall: Address,
    calls: &[(Address, Bytes)],
    block: &str,
) -> GatewayResult<Vec<Result<Bytes, String>>> {
    let input = encode(calls);
    let out: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{ "to": multicall, "input": input }, block]),
    )
    .await?;
    decode(&out, calls.len())
}
