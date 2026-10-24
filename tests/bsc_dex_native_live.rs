//! Файл: tests/bsc_dex_native_live.rs
//! Что делает: нативная монета BNB (решение 62) на ЖИВЫХ ответах узла BSC (tests/data/native_2026-10-25/
//! arb_native_responses_2026-10-25.json, блок 0x7b94144) на вызовы tests/data/native_calls_2026-10-24.json: запросы —
//! ровно наши; WETH() роутера = WBNB; getPair; «кто меняет»; газ симуляции и правило «газ обёртки меньше 0 — 0»;
//! факт exec/native.rs::moved по событиям Deposit (dst = роутер, 0,01) и Withdrawal (src = роутер, 0x183691e0ee19cf);
//! статус нативного перевода (wallet/native_status.rs::judge) — на настоящем выводе BNB с BingX (прямой нативный
//! перевод, сумма зачисления = value транзакции).
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, arb::venues::bsc_dex::{check, exec, wallet, send}.

mod native_common;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256, address};
use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::{Asset, PoolRef, Side};
use arb::venues::bsc_dex::check::simulate::CallResult;
use arb::venues::bsc_dex::check::{emitter, native_probe};
use arb::venues::bsc_dex::exec::{fill, native};
use arb::venues::bsc_dex::wallet::native_status::{self, Tx};
use native_common::*;

const RESPONSES: &str = "tests/data/native_2026-10-25/arb_native_responses_2026-10-25.json";
/// Пара WBNB/USDT PancakeSwap V2 — ответ getPair в записи (тест сверяет).
const PAIR: Address = address!("0x16b9a82891338f9bA80E2D6970FddA79D1eb0daE");

/// Запись ответов. Паника — файла нет.
fn rec() -> Value {
    serde_json::from_str(&std::fs::read_to_string(RESPONSES).unwrap()).unwrap()
}

/// Живые ответы узла BSC (блок 0x7b94144) на вызовы tests/data/native_calls_2026-10-24.json: запросы — ровно наши,
/// WETH() роутера = WBNB, getPair = пара, «кто меняет» — эта пара; симуляция продажи с WRAP_ETH и покупки с UNWRAP_WETH
/// прошла; факт по событиям WBNB Deposit/Withdrawal роутера; нативный перевод — 21000 газа и ни одного события.
#[tokio::test]
async fn recorded_node_responses() {
    let rec = rec();
    let block = rec["block"].as_str().unwrap();
    // Запросы записи — наши вызовы, «latest» заменён номером блока.
    let ours = calls().await;
    for c in ours["calls"].as_array().unwrap().iter().skip(1) {
        let mut want = c.clone();
        for p in want["params"].as_array_mut().unwrap() {
            if p == "latest" {
                *p = json!(block);
            }
        }
        let got = &answer(&rec, c["name"].as_str().unwrap())["request"];
        assert_eq!(got["method"], want["method"]);
        assert_eq!(got["params"], want["params"], "{}", c["name"]);
    }
    let ret = |name: &str| -> Bytes {
        serde_json::from_value(answer(&rec, name)["response"]["result"].clone()).unwrap()
    };
    assert!(!ret("WBNB code").is_empty());
    let weth = arb::venues::bsc_dex::node::abi_dex::IV2Router::WETHCall::abi_decode_returns(&ret(
        "PancakeSwap V2 Router WETH()",
    ))
    .unwrap();
    assert_eq!(weth, WBNB, "dex.native_wrapped = WETH() роутера V2");
    let pair = arb::venues::bsc_dex::pools::abi_v2v3::IV2Factory::getPairCall::abi_decode_returns(
        &ret("PancakeSwap V2 getPair(WBNB, USDT)"),
    )
    .unwrap();
    assert_eq!(pair, PAIR);

    let sim = &answer(&rec, "native wrap/unwrap simulate")["response"]["result"][0]["calls"];
    let calls: Vec<CallResult> = serde_json::from_value(sim.clone()).unwrap();
    assert!(calls.iter().all(|c| c.ok()));
    let gas: Vec<u64> = calls.iter().map(|c| c.gas_used.to::<u64>()).collect();
    assert_eq!(gas, [114_083, 51_591, 105_248, 21_000]);
    // Газ обёртки сверх обмена токеном не бывает меньше 0 (решение 62: завышение — безопасная сторона).
    assert_eq!(native_probe::extra(gas[3], gas[0]), 0);
    assert_eq!(native_probe::extra(gas[0], gas[3]), gas[0] - gas[3]);

    // «Кто меняет» (решение 59): обмен продажи и покупки сделала пара getPair; чужой пул — нет.
    let mut e = entry("pancakeswap_v2", None);
    e.pool.reference = PoolRef::Contract(PAIR);
    let mut other = entry("pancakeswap_v2", None);
    other.pool.reference = PoolRef::Contract(USDT);
    for i in [0, 2] {
        assert!(emitter::missing(&[&e], &calls[i].logs).is_empty());
        assert!(!emitter::missing(&[&other], &calls[i].logs).is_empty());
    }

    // Продажа: роутер обернул ровно value вызова (0,01 BNB), USDT пришли на кошелёк.
    let sell = receipt(&sim[0], block);
    let e16 = U256::from(10u64).pow(U256::from(16u8));
    assert_eq!(native::moved(&sell, WBNB, PCS_UR, Side::Sell), e16);
    assert_eq!(fill::moved(&sell, WBNB, Some(PCS_UR), Some(PAIR)), e16);
    assert!(!fill::moved(&sell, USDT, Some(PAIR), Some(HOLDER)).is_zero());
    // Покупка: роутер развернул ровно полученный от пары WBNB; USDT — 5, из роутера в пару; у кошелька Transfer WBNB нет.
    let buy = receipt(&sim[2], block);
    let got = native::moved(&buy, WBNB, PCS_UR, Side::Buy);
    assert_eq!(got, U256::from(0x18_3691_e0ee_19cf_u64));
    assert_eq!(fill::moved(&buy, WBNB, Some(PAIR), Some(PCS_UR)), got);
    let five = U256::from(5u64) * U256::from(10u64).pow(U256::from(18u8));
    assert_eq!(fill::moved(&buy, USDT, Some(PCS_UR), Some(PAIR)), five);
    assert!(fill::moved(&buy, WBNB, None, Some(HOLDER)).is_zero());
    // Чужой роутер — не наш факт.
    assert!(native::moved(&buy, WBNB, HOLDER, Side::Buy).is_zero());
    // Нативный перевод: событий нет — статус только по самой транзакции (wallet/native_status.rs).
    assert!(calls[3].logs.is_empty());
}

/// Настоящий вывод 0,01998 BNB с BingX (комиссия 0,0005, сеть BEP20, 2026-10-24 12:10:02 — «История вывода» BingX):
/// ответы узла на eth_getTransactionByHash и eth_getTransactionReceipt (tests/data/native_2026-10-25/
/// arb_bingx_bnb_withdraw_2026-10-25.json).
const WITHDRAW: &str = "tests/data/native_2026-10-25/arb_bingx_bnb_withdraw_2026-10-25.json";

/// Статус нативного перевода (wallet/native_status.rs::judge) на настоящем выводе BNB с BingX: это прямой нативный
/// перевод (input пуст, событий нет) — зачисление видно по самой транзакции, сумма зачисления = value транзакции =
/// 0,01998 − 0,0005 = 0,01948 BNB; на другой кошелёк — ошибка с причиной, без догадки.
#[test]
fn bingx_bnb_withdraw_is_direct_native_transfer() {
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(WITHDRAW).unwrap()).unwrap();
    let tx: Tx = serde_json::from_value(answer(&rec, "tx")["response"]["result"].clone()).unwrap();
    let r = receipt_of(&answer(&rec, "receipt")["response"]["result"]);
    assert_eq!(r.status.to::<u64>(), 1);
    assert_eq!(r.gas_used.to::<u64>(), 21_000);
    assert!(r.logs.is_empty(), "нативный перевод — без событий Transfer");
    assert!(tx.input.is_empty());
    let wallet = tx.to.unwrap();
    assert_eq!(
        wallet,
        address!("0x1c8f435a7a86a47490e9c9e15cd1af573985d94d")
    );
    let bnb = Asset {
        ticker: "BNB".into(),
        bsc_contract: Some(WBNB),
    };
    // Зачисление на кошелёк: сумма = value транзакции.
    let (a, wei) = native_status::judge(&tx, wallet, bnb.clone(), true)
        .unwrap()
        .unwrap();
    assert_eq!((a, wei), (bnb.clone(), tx.value));
    assert_eq!(wei, U256::from(19_480_000_000_000_000u64));
    let amount = arb::venues::bsc_dex::num::to_decimal(wei, 18).unwrap();
    assert_eq!(
        amount,
        Decimal::from_str("0.01998").unwrap() - Decimal::from_str("0.0005").unwrap()
    );
    // Зачисление «на другой кошелёк» — не видно: ошибка с причиной.
    assert!(native_status::judge(&tx, HOLDER, bnb.clone(), true).is_err());
    // С точки зрения отправителя (горячий кошелёк BingX) это свой нативный перевод, у чужого — нет.
    assert_eq!(
        native_status::judge(&tx, tx.from, bnb.clone(), false).unwrap(),
        Some((bnb.clone(), wei))
    );
    assert_eq!(native_status::judge(&tx, wallet, bnb, false).unwrap(), None);
}

/// Квитанция из ответа eth_getTransactionReceipt. Паника — не по формату.
fn receipt_of(v: &Value) -> arb::venues::bsc_dex::send::track::Receipt {
    serde_json::from_value(v.clone()).unwrap()
}
