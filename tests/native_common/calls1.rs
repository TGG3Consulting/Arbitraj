//! Файл: tests/native_common/calls1.rs
//! Что делает: первый список ТОЧНЫХ вызовов узла BSC для решения 62 (tests/data/native_calls_2026-10-24.json): код WBNB,
//! WETH() роутера PancakeSwap V2, getPair(WBNB, USDT) и eth_simulateV1 — продажа и покупка BNB через PancakeSwap UR + V2,
//! нативный перевод. План — тем же кодом, что сделка. Записан (tests/data/native_2026-10-25).
//! Кто вызывает: tests/bsc_dex_native.rs, tests/bsc_dex_native_live.rs.
//! От чего зависит: tests/native_common (окружение, план), arb::venues::bsc_dex::{node, pools}.

use alloy::sol_types::SolCall;
use alloy_primitives::{Bytes, U256, address};
use serde_json::{Value, json};

use arb::core::types::Side;

use super::*;

/// Точные вызовы для записи ответов узла (план — тем же кодом, что сделка).
pub async fn calls() -> Value {
    let c = ctx("calls").await;
    let e16 = U256::from(10u64).pow(U256::from(16u8));
    let usdt = U256::from(5u64) * U256::from(10u64).pow(U256::from(18u8));
    let v2 = entry("pancakeswap_v2", None);
    let sell = native_plan(&c, &v2, Side::Sell, e16, HOLDER).await;
    let buy = native_plan(&c, &v2, Side::Buy, usdt, HOLDER).await;
    let erc20 = |data: Vec<u8>| Bytes::from(data);
    let transfer = arb::venues::bsc_dex::node::abi::IERC20::transferCall {
        to: PCS_UR,
        value: usdt,
    };
    let sym = arb::venues::bsc_dex::node::abi_dex::IV2Router::WETHCall {};
    let pair = arb::venues::bsc_dex::pools::abi_v2v3::IV2Factory::getPairCall {
        tokenA: WBNB,
        tokenB: USDT,
    };
    let empty = address!("0x000000000000000000000000000000000000dEaD");
    let sim = json!({
        "blockStateCalls": [{ "calls": [
            { "from": HOLDER, "to": PCS_UR, "input": sell.data(U256::ZERO), "value": sell.value() },
            { "from": HOLDER, "to": USDT, "input": erc20(transfer.abi_encode()) },
            { "from": HOLDER, "to": PCS_UR, "input": buy.data(U256::ZERO) },
            { "from": HOLDER, "to": empty, "input": "0x", "value": U256::from(10u64).pow(U256::from(15u8)) }
        ]}],
        "validation": false
    });
    json!({
        "about": "Точные вызовы узла BSC для решения 62 (BNB ↔ WBNB, tests/bsc_dex_native.rs). Сначала eth_blockNumber, затем остальные на том же блоке (вместо \"latest\" — номер из первого ответа). Ответы — как вернул узел: {name, request, response}. Симуляция: продажа 0,01 BNB с WRAP_ETH → USDT; перевод 5 USDT на Universal Router; покупка WBNB за эти USDT с UNWRAP_WETH на кошелёк; нативный перевод 0,001 BNB на пустой адрес. Нужны status, gasUsed, logs (с data) каждого вызова. Если у HOLDER нет BNB или 5 USDT — заменить HOLDER на любой адрес с ними во всех вызовах (например кошелёк бота) и сообщить.",
        "calls": [
            { "name": "block", "method": "eth_blockNumber", "params": [] },
            { "name": "WBNB code", "method": "eth_getCode", "params": [WBNB, "latest"] },
            { "name": "PancakeSwap V2 Router WETH()", "method": "eth_call", "params": [{ "to": PCS_V2_ROUTER, "input": erc20(sym.abi_encode()) }, "latest"] },
            { "name": "PancakeSwap V2 getPair(WBNB, USDT)", "method": "eth_call", "params": [{ "to": PCS_V2_FACTORY, "input": erc20(pair.abi_encode()) }, "latest"] },
            { "name": "native wrap/unwrap simulate", "method": "eth_simulateV1", "params": [sim, "latest"] }
        ]
    })
}
