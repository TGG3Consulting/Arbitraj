//! Файл: venues/bsc_dex/quote/virtual_quoter.rs
//! Что делает: решение 186 (Тигран, 05.11.2026: «нам нужны все DEX») — «виртуальный квотер» для DEX без своего
//! квотера в документации (форки V3, Algebra, Slipstream; SheepDex — обратный вызов swapCallback, решение 274; SquadSwap V3 — squadV3SwapCallback, решение 281; Metric OMM — quoteMetric + metricOmmSwapCallback, решение 285). Код контракта contracts/VirtualQuoter.sol (собран
//! contracts/build.sh, артефакт contracts/out/VirtualQuoter.bin-runtime, хеш сверяется с VirtualQuoter.sha256)
//! подставляется по своему адресу в eth_call переопределением кода счёта (state override, 3-й параметр eth_call geth:
//! https://geth.ethereum.org/docs/interacting-with-geth/rpc/objects#state-override-set). Квотер зовёт swap САМОГО пула,
//! пул исполняет свой настоящий код и вызывает обратный вызов квотера, тот откатывает всё с дельтами — выход обмена
//! «по ответу сети», как у официального QuoterV2, но без поиска пула по фабрике. В сеть ничего не уходит.
//! Пул взял меньше входа — ликвидности до предела цены не хватило: это «не хватает ликвидности» (как SPL у QuoterV2).
//! Включается настройкой dex.quoter_virtual [DEX] = да; не задана или «нет» — как раньше (квотер DEX из dex.quoter).
//! Кто вызывает: quote/quoter.rs (out, out_at — решение 261), pools/families.rs (required — dex.quoter не нужен), exec/dry_surplus_tail.rs
//! (data, address, runtime — квотер в eth_simulateV1, решение 252 вариант «б»).
//! От чего зависит: node/rpc.rs, pools/registry.rs, pools/families.rs (Proto), sha2, contracts/out/VirtualQuoter.*.
//! Настройки: dex.quoter_virtual [DEX] (необязательная).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256, keccak256};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::core::types::{PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::super::pools::families::Proto;
use super::super::pools::registry::Entry;

alloy::sol! {
    function quoteExactIn(address pool, bool zeroForOne, uint256 amountIn) external returns (uint256 used, uint256 out);
    /// Решение 285: пул Metric OMM (swap(address,bool,int128,uint128,bytes,bytes) + metricOmmSwapCallback); ответ — тот же.
    function quoteMetric(address pool, bool zeroForOne, uint256 amountIn) external returns (uint256 used, uint256 out);
}

/// Код в сети (deployedBytecode solc).
const RUNTIME: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/contracts/out/VirtualQuoter.bin-runtime"
));
/// Хеши артефактов (строка «<sha256>  <файл>»).
const SHA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/contracts/out/VirtualQuoter.sha256"
));

/// Текст «не хватает ликвидности» — тот же смысл, что SPL у QuoterV2 (quote/quoter_revert.rs).
pub const SHORT: &str =
    "виртуальный квотер: пул взял не весь вход — ликвидности до предела цены не хватает";

/// Виды пулов, чей swap вызывает uniswapV3SwapCallback / pancakeV3SwapCallback / algebraSwapCallback (или swapCallback
/// форка SheepDex — вид v3_fork, решение 274) с подписью
/// swap(address,bool,int256,uint160,bytes) — их квотер умеет. Ошибок нет.
pub fn supports(p: Proto) -> bool {
    matches!(
        p,
        Proto::UniswapV3
            | Proto::PancakeV3
            | Proto::Algebra
            | Proto::AlgebraIntegral
            | Proto::Slipstream
            | Proto::MetricOmm
    )
}

/// Включён ли виртуальный квотер у DEX: dex.quoter_virtual = да и вид поддержан. Ошибок нет (не задано — нет).
pub fn on(ctx: &Ctx, dex_id: &str, p: Proto) -> bool {
    supports(p) && ctx.dex_flag("dex.quoter_virtual", dex_id).unwrap_or(false)
}

/// Адрес, по которому подставляется код: последние 20 байт keccak256("arb.virtual_quoter") — не адрес чьего-то
/// счёта и не настройка (в сети по нему кода нет, подмена действует только внутри одного eth_call). Ошибок нет.
pub fn address() -> Address {
    Address::from_slice(&keccak256(b"arb.virtual_quoter")[12..])
}

/// Код квотера после сверки SHA-256 с VirtualQuoter.sha256. Ошибки (текст): хеш не записан или не совпал, не hex.
pub fn runtime() -> Result<Bytes, String> {
    let name = "VirtualQuoter.bin-runtime";
    let have = hex::encode(Sha256::digest(RUNTIME.as_bytes()));
    let want = SHA
        .lines()
        .filter_map(|l| l.split_once("  "))
        .find(|(_, n)| *n == name)
        .map(|(h, _)| h);
    match want {
        Some(w) if w == have => {}
        Some(w) => {
            return Err(format!(
                "contracts/out/{name}: SHA-256 {have} ≠ записанному {w} — пересоберите contracts/build.sh"
            ));
        }
        None => {
            return Err(format!(
                "contracts/out/VirtualQuoter.sha256: нет хеша {name}"
            ));
        }
    }
    hex::decode(RUNTIME.trim())
        .map(Bytes::from)
        .map_err(|e| format!("contracts/out/{name}: не hex ({e})"))
}

/// Разбор ответа: (взял входа, отдаст). Принимает: сырой ответ, сумму входа. Возвращает: выход или текст причины
/// (SHORT — взял меньше входа). Ошибок сверх этого нет; None — ответ не по ABI.
pub fn decode(raw: &[u8], amount: U256) -> Option<Result<U256, &'static str>> {
    let r = quoteExactInCall::abi_decode_returns(raw).ok()?;
    Some(if r.used < amount {
        Err(SHORT)
    } else {
        Ok(r.out)
    })
}

/// Вход вызова quoteExactIn (пул, направление, сумма) — для eth_call и для eth_simulateV1 с подменой кода по address()
/// (exec/dry_surplus_cut.rs). Ошибок нет.
pub fn data(pool: Address, zero_for_one: bool, amount: U256) -> Bytes {
    Bytes::from(
        quoteExactInCall {
            pool,
            zeroForOne: zero_for_one,
            amountIn: amount,
        }
        .abi_encode(),
    )
}

/// Вход вызова quoteMetric (решение 285: пул Metric OMM; zero_for_one — в порядке токенов САМОГО пула). Ошибок нет.
pub fn data_metric(pool: Address, zero_for_one: bool, amount: U256) -> Bytes {
    Bytes::from(
        quoteMetricCall {
            pool,
            zeroForOne: zero_for_one,
            amountIn: amount,
        }
        .abi_encode(),
    )
}

/// Пул и вход вызова квотера для записи пула (V3-подобные — quoteExactIn по token0 пары; Metric — quoteMetric по
/// token0 самого пула, OraclePmmData.pool_token0). Принимает: запись, токен входа, сумму. Возвращает: (пул, данные) или
/// текст причины (пул не контракт; у пула Metric не прочитан порядок токенов). Ошибок иных нет.
pub fn request(e: &Entry, token_in: Address, amount: U256) -> Result<(Address, Bytes), &'static str> {
    let PoolRef::Contract(pool) = e.pool.reference else {
        return Err("виртуальный квотер: пул не контракт");
    };
    if e.family.proto == Proto::MetricOmm {
        let Some(t0) = (match &e.pool.state {
            PoolState::OraclePmm(d) => d.pool_token0,
            _ => None,
        }) else {
            return Err("виртуальный квотер: у пула Metric не прочитан порядок токенов");
        };
        return Ok((pool, data_metric(pool, token_in == t0, amount)));
    }
    Ok((pool, data(pool, token_in == e.pool.token0, amount)))
}

/// Выход пула на точный вход по его собственному коду. Принимает: окружение, запись пула, токен входа, сумму.
/// Ошибки: Internal (код квотера, ссылка пула не контракт, вид не поддержан), узел (откат пула — его текст),
/// BadResponse (ответ не по ABI), SwapReverted с текстом SHORT — ликвидности не хватает.
pub async fn out(ctx: &Ctx, e: &Entry, token_in: Address, amount: U256) -> GatewayResult<U256> {
    out_at(ctx, e, token_in, amount, "latest").await
}

/// Как out, но на заданном блоке (решение 261: периодическая сверка — на блоке состояния пула). Принимает: ещё блок
/// (номер hex или «latest»). Ошибки: как у out; StaleData — состояния блока у узла нет.
pub async fn out_at(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
    block: &str,
) -> GatewayResult<U256> {
    if !supports(e.family.proto) {
        return Err(ctx.err(
            ErrorCode::Internal,
            format!(
                "{}: виртуальный квотер не умеет этот вид пула",
                e.family.title
            ),
        ));
    }
    let (_, data) = request(e, token_in, amount).map_err(|m| ctx.err(ErrorCode::Internal, m))?;
    let code = runtime().map_err(|m| ctx.err(ErrorCode::Internal, m))?;
    let at = address();
    let raw: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([
            { "to": at, "input": data },
            block,
            { at.to_string(): { "code": code } }
        ]),
    )
    .await?;
    match decode(&raw, amount) {
        Some(Ok(v)) => Ok(v),
        Some(Err(why)) => Err(ctx.err(ErrorCode::SwapReverted, why)),
        None => Err(ctx.err(
            ErrorCode::BadResponse,
            format!("{}: ответ виртуального квотера не по ABI", e.family.title),
        )),
    }
}
