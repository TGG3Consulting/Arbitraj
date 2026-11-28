//! Сверка балансов и события Trade по EIP-1898 и eth_simulateV1 на узле площадки, без state overrides.
use super::super::node::rpc;
use super::{
    Ctx,
    wire::{Firm, Trade},
};
use super::model::Request;
use alloy::sol_types::SolEvent;
use super::router_identity::{authorized, role_calls};
use alloy_primitives::{Address, B256, Bytes, U64, U256, keccak256};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub(super) struct Header {
    pub hash: B256,
    pub number: U64,
    pub timestamp: U64,
}

/// Закрепляет каноническую голову; chain ID самого узла сверяет с ожидаемым вызывающий.
pub(super) async fn header(ctx: &Ctx, chain: u64) -> Result<Header, String> {
    let id: U64 = rpc::call(ctx, "eth_chainId", json!([]))
        .await
        .map_err(|_| "Узел не подтвердил eth_chainId")?;
    if id.to::<u64>() != chain {
        return Err("Узел принадлежит другой сети".into());
    }
    rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false]))
        .await
        .map_err(|_| "Узел не вернул каноническую голову".into())
}

pub(super) fn tag(h: B256) -> Value {
    json!({"blockHash":h,"requireCanonical":true})
}

/// decimals токена на том же хеше, для нативной валюты четырёх EVM применяется 18.
pub(super) async fn decimals(ctx: &Ctx, token: Address, h: B256) -> Result<u8, String> {
    if token.is_zero() {
        return Ok(18);
    }
    let out: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":token,"data":"0x313ce567"},tag(h)]),
    )
    .await
    .map_err(|_| "Узел не подтвердил decimals токена")?;
    if out.len() != 32 {
        return Err("decimals вернул некорректный ABI".into());
    }
    u8::try_from(U256::from_be_slice(&out)).map_err(|_| "decimals превышает uint8".into())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Block {
    parent_hash: B256,
    timestamp: U64,
    calls: Vec<Call>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Call {
    status: U64,
    return_data: Bytes,
    gas_used: U64,
    logs: Vec<Log>,
}
#[derive(Deserialize)]
struct Log {
    address: Address,
    topics: Vec<B256>,
    data: Bytes,
}

/// Формирует balanceOf для ERC-20 либо getEthBalance у настроенного Multicall3.
fn balance(ctx: &Ctx, token: Address, who: Address) -> Result<Value, String> {
    let (to, sig) = if token.is_zero() {
        let address = ctx
            .text("dex.multicall3")
            .map_err(|e| e.message)?
            .parse::<Address>()
            .map_err(|_| "Неверный Multicall3")?;
        (address, "getEthBalance(address)")
    } else {
        (token, "balanceOf(address)")
    };
    let mut data = keccak256(sig.as_bytes())[..4].to_vec();
    data.extend_from_slice(&[0; 12]);
    data.extend_from_slice(who.as_slice());
    Ok(json!({"from":who,"to":to,"input":Bytes::from(data)}))
}

/// Точное событие официального пула; темы Trade не индексированы по ABI IHashflowPool.
pub(super) fn event_matches(
    log: &[u8],
    req: &Request,
    txid: B256,
    amount: U256,
    out: U256,
) -> bool {
    let Ok(e) = Trade::decode_raw_log([Trade::SIGNATURE_HASH], log) else {
        return false;
    };
    e.trader == req.trader
        && e.effectiveTrader == req.trader
        && e.txid == txid
        && e.baseToken == req.input
        && e.quoteToken == req.output
        && e.baseTokenAmount == amount
        && e.quoteTokenAmount == out
}

/// Не передаёт наружу подписанную calldata или сырой RPC-ответ. Ошибка баланса, события или канона закрывает маршрут.
pub(super) async fn run(
    ctx: &Ctx,
    head: B256,
    req: &Request,
    firm: &Firm,
    amount: U256,
    out: U256,
    value: U256,
) -> Result<U64, String> {
    let bi = balance(ctx, req.input, req.trader)?;
    let bo = balance(ctx, req.output, req.trader)?;
    let swap =
        json!({"from":req.trader,"to":firm.target_contract,"value":value,"input":firm.calldata});
    let mut calls = vec![bi.clone(), bo.clone(), swap, bi, bo];
    let expected = ctx.chain_id().and_then(|id| super::deployments().ok()?.router_runtime.get(&id));
    if let Some(hash) = expected {
        let code: Bytes = rpc::call(ctx, "eth_getCode", json!([firm.target_contract,tag(head)]))
            .await.map_err(|_| "Узел не подтвердил код Router Hashflow")?;
        if keccak256(&code) != *hash {
            return Err("Изменился код Router Hashflow".into());
        }
        // Читаем роли до денежных вызовов в том же измерении.
        calls.splice(0..0,role_calls(firm.target_contract,firm.quote_data.pool));
    }
    let block = json!({"calls":calls});
    let blocks: Vec<Block> = rpc::call(
        ctx,
        "eth_simulateV1",
        json!([{"blockStateCalls":[block]},tag(head)]),
    )
    .await
    .map_err(|_| "Узел не подтвердил симуляцию RFQ на закреплённом блоке")?;
    let [b] = blocks.as_slice() else {
        return Err("Неверное число блоков симуляции".into());
    };
    let payments = if expected.is_some() {
        let [permitted, pool_router, payments @ ..] = b.calls.as_slice() else {
            return Err("Нет обеих ролей Hashflow в симуляции".into());
        };
        if !authorized(firm.target_contract, &permitted.return_data, &pool_router.return_data) {
            return Err("Подписанный пул и Router Hashflow не подтвердили взаимную регистрацию".into());
        }
        payments
    } else {
        b.calls.as_slice()
    };
    let [before_in, before_out, execution, after_in, after_out] = payments else {
        return Err("Неполный ответ симуляции".into());
    };
    if b.parent_hash != head
        || b.timestamp.to::<u64>() >= firm.quote_data.quote_expiry
        || b.calls.iter().any(|r| r.status.to::<u64>() != 1)
    {
        return Err("Симуляция RFQ откатилась, истекла или использовала другую голову".into());
    }
    let bal = |r: &Call| {
        (r.return_data.len() == 32)
            .then(|| U256::from_be_slice(&r.return_data))
            .ok_or("Некорректный баланс в симуляции")
    };
    if bal(after_out)?.checked_sub(bal(before_out)?) != Some(out)
        || (!req.input.is_zero() && bal(before_in)?.checked_sub(bal(after_in)?) != Some(amount))
    {
        return Err("Баланс трейдера расходится с подписанными суммами".into());
    }
    let mut trades = execution.logs.iter().filter(|l| {
        l.address == firm.quote_data.pool && l.topics.as_slice() == [Trade::SIGNATURE_HASH]
    });
    if !trades
        .next()
        .is_some_and(|l| event_matches(&l.data, req, firm.quote_data.txid, amount, out))
        || trades.next().is_some()
    {
        return Err("Нет однозначного подтверждения Trade от подписанного пула".into());
    }
    Ok(execution.gas_used)
}
