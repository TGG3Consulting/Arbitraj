//! Файл: tests/bsc_dex_native_v3_live.rs
//! Что делает: нативная монета BNB (решение 62) через пулы V3 — на ЖИВЫХ ответах узла BSC (tests/data/native_2026-10-25/
//! arb_native_responses_2_2026-10-25.json, блок 0x7b9607f) на вызовы tests/data/native_calls_2_2026-10-25.json:
//! запросы — ровно наши; getPool фабрик V3 PancakeSwap и Uniswap; код Universal Router Uniswap есть; продажа с WRAP_ETH
//! и покупка с UNWRAP_WETH через UR PancakeSwap и UR Uniswap прошли; обмен сделал пул getPool на 500 («кто меняет»);
//! факт exec/native.rs::moved по Deposit/Withdrawal WBNB своего роутера; газ симуляции.
//! Плюс ответ GeckoTerminal по пулам WBNB (страница 1): пулов Infinity CL WBNB/USDT нет — путь BNB через V4 / Infinity
//! закрыт (exec/native.rs::pool_ok).
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, arb::venues::bsc_dex::{check, exec, directory, pools}.

mod native_common;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};

use arb::core::types::{PoolRef, Side};
use arb::venues::bsc_dex::check::emitter;
use arb::venues::bsc_dex::check::simulate::CallResult;
use arb::venues::bsc_dex::directory::gt_parse;
use arb::venues::bsc_dex::exec::{fill, native};
use arb::venues::bsc_dex::pools::abi_v2v3::IV3Factory;
use native_common::*;

const RESPONSES: &str = "tests/data/native_2026-10-25/arb_native_responses_2_2026-10-25.json";
const GECKO: &str = "tests/data/native_2026-10-25/arb_gecko_wbnb_pools_2026-10-25.json";

/// Файл записи. Паника — нет файла.
fn load(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Адрес из ответа getPool. Паника — не по ABI.
fn pool(rec: &Value, name: &str) -> Address {
    let b: Bytes = serde_json::from_value(answer(rec, name)["response"]["result"].clone()).unwrap();
    IV3Factory::getPoolCall::abi_decode_returns(&b).unwrap()
}

#[tokio::test]
async fn recorded_v3_through_both_routers() {
    let rec = load(RESPONSES);
    let block = rec["block"].as_str().unwrap();
    for c in calls2::calls2().await["calls"]
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
    {
        let mut want = c.clone();
        for p in want["params"].as_array_mut().unwrap() {
            if p == "latest" {
                *p = json!(block);
            }
        }
        let got = &answer(&rec, c["name"].as_str().unwrap())["request"];
        assert_eq!(got["params"], want["params"], "{}", c["name"]);
    }
    let code: Bytes = serde_json::from_value(
        answer(&rec, "Uniswap Universal Router code")["response"]["result"].clone(),
    )
    .unwrap();
    assert!(!code.is_empty());
    let pcs = pool(&rec, "PancakeSwap V3 getPool(WBNB, USDT, 500)");
    let uni = pool(&rec, "Uniswap V3 getPool(WBNB, USDT, 500)");
    for name in [
        "PancakeSwap V3 getPool(WBNB, USDT, 100)",
        "Uniswap V3 getPool(WBNB, USDT, 3000)",
    ] {
        assert_ne!(pool(&rec, name), Address::ZERO);
    }
    let sim = &answer(&rec, "native V3 wrap/unwrap simulate")["response"]["result"][0]["calls"];
    let calls: Vec<CallResult> = serde_json::from_value(sim.clone()).unwrap();
    assert!(calls.iter().all(|c| c.ok()));
    let gas: Vec<u64> = calls.iter().map(|c| c.gas_used.to::<u64>()).collect();
    assert_eq!(gas, [148_706, 51_591, 126_694, 112_643, 51_603, 113_846]);
    let e16 = U256::from(10u64).pow(U256::from(16u8));
    let five = U256::from(5u64) * U256::from(10u64).pow(U256::from(18u8));
    for (i, (kind, router, addr)) in [("pancakeswap_v3", PCS_UR, pcs), ("uniswap_v3", UNI_UR, uni)]
        .into_iter()
        .enumerate()
    {
        let (s, b) = (3 * i, 3 * i + 2);
        // «Кто меняет»: обмен сделал пул getPool на 500, а не соседний уровень.
        let mut e = entry(kind, Some(500));
        e.pool.reference = PoolRef::Contract(addr);
        let mut other = entry(kind, Some(500));
        other.pool.reference = PoolRef::Contract(USDT);
        for k in [s, b] {
            assert!(emitter::missing(&[&e], &calls[k].logs).is_empty(), "{kind}");
            assert!(
                !emitter::missing(&[&other], &calls[k].logs).is_empty(),
                "{kind}"
            );
        }
        // Продажа: свой роутер обернул ровно 0,01 BNB и отдал их пулу; USDT пришли на кошелёк.
        let sell = receipt(&sim[s], block);
        assert_eq!(
            native::moved(&sell, WBNB, router, Side::Sell),
            e16,
            "{kind}"
        );
        assert_eq!(fill::moved(&sell, WBNB, Some(router), Some(addr)), e16);
        assert!(!fill::moved(&sell, USDT, Some(addr), Some(HOLDER)).is_zero());
        // Покупка: свой роутер развернул ровно полученный от пула WBNB; USDT — 5, из роутера в пул.
        let buy = receipt(&sim[b], block);
        let got = native::moved(&buy, WBNB, router, Side::Buy);
        assert!(!got.is_zero());
        assert_eq!(
            fill::moved(&buy, WBNB, Some(addr), Some(router)),
            got,
            "{kind}"
        );
        assert_eq!(fill::moved(&buy, USDT, Some(router), Some(addr)), five);
        assert!(fill::moved(&buy, WBNB, None, Some(HOLDER)).is_zero());
    }
    // Факт по чужому роутеру — ноль (события своего роутера не спутать).
    assert!(native::moved(&receipt(&sim[0], block), WBNB, UNI_UR, Side::Sell).is_zero());
}

/// GeckoTerminal, пулы WBNB (страница 1): пулы WBNB/USDT — только V2 и V3 (PancakeSwap, Uniswap); Infinity CL с WBNB/USDT
/// нет — путь BNB через V4 / Infinity закрыт (решение 62).
#[test]
fn gecko_has_no_infinity_wbnb_usdt() {
    let rec = load(GECKO);
    let list = gt_parse::pools(rec["response"]["body"].as_str().unwrap()).unwrap();
    assert_eq!(list.len(), 20);
    let pair: Vec<&str> = list
        .iter()
        .filter(|p| (p.base == WBNB && p.quote == USDT) || (p.base == USDT && p.quote == WBNB))
        .map(|p| p.dex_id.as_str())
        .collect();
    assert!(!pair.is_empty());
    assert!(pair.iter().all(|d| !d.contains("infinity")), "{pair:?}");
}
