//! Файл: venues/kyberswap/node.rs
//! Что делает: тонкий клиент JSON-RPC узла BSC по HTTP для ИСПОЛНЕНИЯ (цены — из агрегатора, узел для них не нужен):
//! один запрос с приведением result к типу, eth_call к контракту (ABI), цена газа, оценка газа, отправка подписанной
//! транзакции, квитанция. Своя копия адаптера kyberswap (изоляция: не импортирует bsc_dex), на alloy напрямую.
//! Кто вызывает: balances.rs, nonce.rs, trade.rs, transfer.rs, tx.rs, connection.rs.
//! От чего зависит: ctx.rs (адрес узла с ключом, пределы HTTP), errors.rs, gateway/http.rs, alloy.
//! Настройки: kyber.node_http_url, kyber.http_timeout_ms, http.* ; секрет kyber.node_http_key (если в адресе {key}).
//!
//! Методы и формат — JSON-RPC 2.0 (https://www.jsonrpc.org/specification) и execution-apis
//! https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth
//! (eth_call, eth_gasPrice, eth_estimateGas, eth_sendRawTransaction, eth_getTransactionReceipt, eth_blockNumber,
//! eth_getBalance, eth_getTransactionCount, eth_chainId). Chain ID BSC mainnet = 56 (docs.bnbchain.org).

use alloy::rpc::types::Log;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U64, U256};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::http;

use super::ctx::Ctx;
use super::{errors, num};

/// Chain ID сети BSC mainnet — значение протокола (https://docs.bnbchain.org): подпись EIP-155 и сверка узла.
pub const BSC_CHAIN_ID: u64 = 56;
/// Источник ошибок для текста причины.
pub const SOURCE: &str = "узел BSC (HTTP)";

/// Квитанция транзакции: только нужные поля (execution-apis transaction.yaml#L127-L165).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub status: U64,
    pub block_number: U64,
    pub gas_used: U64,
    pub effective_gas_price: U256,
    pub logs: Vec<Log>,
}

impl Receipt {
    /// Газ транзакции в BNB. None — не помещается в Decimal.
    pub fn gas_bnb(&self) -> Option<Decimal> {
        num::to_decimal(
            self.effective_gas_price
                .saturating_mul(U256::from(self.gas_used)),
            num::BNB_DECIMALS,
        )
    }
}

/// Один запрос к узлу с приведением result к типу.
/// Принимает: метод, параметры (массив JSON). Возвращает: значение result.
/// Ошибки: NotConfigured/SecretsLocked (адрес, ключ), Timeout/Unreachable, ответ-ошибка узла, BadResponse.
pub async fn call<T: DeserializeOwned>(ctx: &Ctx, method: &str, params: Value) -> GatewayResult<T> {
    let url = ctx.node_url()?;
    let lim = ctx.limits()?;
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let req = ctx.http.post(url.as_str()).json(&body);
    let (status, text) = http::exchange(req, lim)
        .await
        .map_err(|f| errors::from_fail(&f, &lim, SOURCE))?;
    if !status.is_success() {
        // Узел мог ответить телом JSON-RPC с ошибкой и не-2xx — разбираем его первым.
        if let Some(e) = rpc_error(&text) {
            return Err(e);
        }
        return Err(errors::from_http(status.as_u16(), SOURCE, &text));
    }
    if let Some(e) = rpc_error(&text) {
        return Err(e);
    }
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| errors::bad(&format!("не JSON: {e}"), SOURCE, &text))?;
    let result = v
        .get("result")
        .ok_or_else(|| errors::bad("нет поля result", SOURCE, &text))?;
    serde_json::from_value(result.clone())
        .map_err(|e| errors::bad(&format!("неожиданный вид result: {e}"), SOURCE, &text))
}

/// Если в тексте ответа есть объект ошибки JSON-RPC — единая ошибка шлюза; иначе None. Ошибок нет.
fn rpc_error(text: &str) -> Option<crate::gateway::errors::GatewayError> {
    let v: Value = serde_json::from_str(text).ok()?;
    let e = v.get("error")?;
    let code = e.get("code").and_then(Value::as_i64).unwrap_or(0);
    let msg = e.get("message").and_then(Value::as_str).unwrap_or_default();
    Some(errors::from_rpc(code, msg, SOURCE, text))
}

/// Вызов view-функции контракта (eth_call на «latest»). Принимает: адрес, вызов (sol!).
/// Возвращает: разобранный ответ. Ошибки: ответ-ошибка узла (откат — SwapReverted), BadResponse.
pub async fn view<C: SolCall>(ctx: &Ctx, to: Address, c: &C) -> GatewayResult<C::Return> {
    let data = Bytes::from(c.abi_encode());
    let out: Bytes = call(
        ctx,
        "eth_call",
        json!([{ "to": to, "input": data }, "latest"]),
    )
    .await?;
    C::abi_decode_returns(out.as_ref()).map_err(|e| {
        errors::bad(
            &format!("ответ {} не по ABI: {e}", C::SIGNATURE),
            SOURCE,
            &alloy_primitives::hex::encode_prefixed(&out),
        )
    })
}

/// Номер последнего блока (eth_blockNumber). Ошибки: узел.
pub async fn block_number(ctx: &Ctx) -> GatewayResult<u64> {
    let n: U64 = call(ctx, "eth_blockNumber", json!([])).await?;
    Ok(n.to())
}

/// Цена газа с надбавкой kyber.gas_price_markup_pct (быстрее в блок). Ошибки: узел, NotConfigured.
pub async fn gas_price(ctx: &Ctx) -> GatewayResult<U256> {
    let raw: U256 = call(ctx, "eth_gasPrice", json!([])).await?;
    let markup = ctx.dec("kyber.gas_price_markup_pct")?;
    let bumped = num::to_decimal(raw, 0)
        .map(|d| d * (Decimal::ONE + markup / Decimal::ONE_HUNDRED))
        .and_then(|d| num::to_units(d.ceil(), 0));
    Ok(bumped.unwrap_or(raw))
}

/// Лимит газа: eth_estimateGas × (1 + kyber.gas_limit_margin_pct/100). Проверяет, что вызов не откатится.
/// Принимает: отправителя, адрес, данные, сумму BNB (wei). Ошибки: откат (текст узла), NotConfigured, BadResponse.
pub async fn estimate_gas(
    ctx: &Ctx,
    from: Address,
    to: Address,
    data: &Bytes,
    value: U256,
) -> GatewayResult<u64> {
    let est: U64 = call(
        ctx,
        "eth_estimateGas",
        json!([{ "from": from, "to": to, "input": data, "value": value }]),
    )
    .await?;
    let margin = ctx.dec("kyber.gas_limit_margin_pct")?;
    let g = Decimal::from(est.to::<u64>()) * (Decimal::ONE + margin / Decimal::ONE_HUNDRED);
    u64::try_from(g.ceil()).map_err(|_| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Лимит газа {g} не помещается"),
        )
    })
}

/// Количество транзакций адреса на «latest» (nonce). Ошибки: узел.
pub async fn tx_count(ctx: &Ctx, who: Address) -> GatewayResult<u64> {
    let n: U64 = call(ctx, "eth_getTransactionCount", json!([who, "latest"])).await?;
    Ok(n.to())
}

/// Баланс BNB адреса в wei (eth_getBalance). Ошибки: узел.
pub async fn balance(ctx: &Ctx, who: Address) -> GatewayResult<U256> {
    call(ctx, "eth_getBalance", json!([who, "latest"])).await
}

/// Отправляет подписанную транзакцию (eth_sendRawTransaction). Возвращает: хеш. Ошибки: узел (nonce, газ, откат).
pub async fn send_raw(ctx: &Ctx, raw: &Bytes) -> GatewayResult<B256> {
    call(ctx, "eth_sendRawTransaction", json!([raw])).await
}

/// Квитанция транзакции по хешу. None — ещё не в блоке. Ошибки: узел.
pub async fn receipt(ctx: &Ctx, hash: B256) -> GatewayResult<Option<Receipt>> {
    call(ctx, "eth_getTransactionReceipt", json!([hash])).await
}
