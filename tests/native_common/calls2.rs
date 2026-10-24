//! Файл: tests/native_common/calls2.rs
//! Что делает: второй список ТОЧНЫХ вызовов узла BSC для решения 62 (tests/data/native_calls_2_2026-10-25.json, просьба
//! ревью #50): продажа и покупка BNB через один пул V3 WBNB/USDT (комиссия 500) — Universal Router PancakeSwap и
//! Universal Router Uniswap (2.0), план — тем же кодом, что сделка. Перед симуляцией — getPool фабрик V3 (пул на 500
//! и соседний уровень — какой пул менял, тест сверит по событиям «кто меняет») и код Universal Router Uniswap.
//! Infinity CL сюда не входит: Universal Router нужен полный ключ пула (валюты, крючок, менеджер, комиссия, parameters),
//! а пула WBNB/USDT Infinity CL нет ни в одной записи — сначала нужен ответ индексатора (PoolId) и poolIdToPoolKey.
//! Кто вызывает: tests/native_common/mod.rs (calls2), tests/bsc_dex_native.rs.
//! От чего зависит: tests/native_common (окружение, план), arb::venues::bsc_dex::pools::abi_v2v3.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256, aliases::U24};
use serde_json::{Value, json};

use arb::core::types::Side;
use arb::venues::bsc_dex::pools::abi_v2v3::IV3Factory;

use super::*;

/// eth_call getPool(WBNB, USDT, fee) фабрики. Ошибок нет.
fn get_pool(name: &str, factory: Address, fee: u32) -> Value {
    let data = IV3Factory::getPoolCall {
        tokenA: WBNB,
        tokenB: USDT,
        fee: U24::from(fee),
    }
    .abi_encode();
    json!({ "name": name, "method": "eth_call", "params": [{ "to": factory, "input": Bytes::from(data) }, "latest"] })
}

/// Список вызовов (тот же формат, что native_calls_2026-10-24.json).
pub async fn calls2() -> Value {
    let c = ctx("calls2").await;
    let e16 = U256::from(10u64).pow(U256::from(16u8));
    let usdt = U256::from(5u64) * U256::from(10u64).pow(U256::from(18u8));
    let mut sim = Vec::new();
    for (kind, router) in [("pancakeswap_v3", PCS_UR), ("uniswap_v3", UNI_UR)] {
        let e = entry(kind, Some(500));
        let sell = native_plan(&c, &e, Side::Sell, e16, HOLDER).await;
        let buy = native_plan(&c, &e, Side::Buy, usdt, HOLDER).await;
        sim.extend([
            json!({ "from": HOLDER, "to": router, "input": sell.data(U256::ZERO), "value": sell.value() }),
            json!({ "from": HOLDER, "to": USDT, "input": usdt_to(router, usdt) }),
            json!({ "from": HOLDER, "to": router, "input": buy.data(U256::ZERO) }),
        ]);
    }
    let sim = json!({ "blockStateCalls": [{ "calls": sim }], "validation": false });
    json!({
        "about": "Точные вызовы узла BSC для решения 62, часть 2 (tests/native_common/calls2.rs): V3 WBNB/USDT (комиссия 500) через Universal Router PancakeSwap (вызовы 0–2 симуляции) и Universal Router Uniswap 2.0 (3–5): продажа 0,01 BNB с WRAP_ETH → USDT; перевод 5 USDT на роутер; покупка WBNB за них с UNWRAP_WETH на кошелёк. Сначала eth_blockNumber, затем остальные на том же блоке (вместо \"latest\" — номер из первого ответа). Ответы — как вернул узел: {name, request, response}; у симуляции нужны status, gasUsed, logs (с data) каждого вызова. Если код Universal Router Uniswap пуст или getPool на 500 — нулевой адрес, сообщить (адрес роутера или уровень комиссии поменяю). Infinity CL — следующим списком: нужен ключ пула WBNB/USDT (его нет в записях).",
        "calls": [
            { "name": "block", "method": "eth_blockNumber", "params": [] },
            get_pool("PancakeSwap V3 getPool(WBNB, USDT, 500)", PCS_V3_FACTORY, 500),
            get_pool("PancakeSwap V3 getPool(WBNB, USDT, 100)", PCS_V3_FACTORY, 100),
            get_pool("Uniswap V3 getPool(WBNB, USDT, 500)", UNI_V3_FACTORY, 500),
            get_pool("Uniswap V3 getPool(WBNB, USDT, 3000)", UNI_V3_FACTORY, 3000),
            { "name": "Uniswap Universal Router code", "method": "eth_getCode", "params": [UNI_UR, "latest"] },
            { "name": "native V3 wrap/unwrap simulate", "method": "eth_simulateV1", "params": [sim, "latest"] }
        ]
    })
}
