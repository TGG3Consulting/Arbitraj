//! Файл: tests/bsc_dex_service_tax.rs
//! Что делает: налоги служебной монеты USDT (решение 67, check/service_tax.rs). Живой бот (main 43bda1f): «USDC bingx →
//! bsc_dex: отказ — bsc_dex: стоимость возврата USDT: USDT: налоги не проверены» — налог USDT никто не проверял.
//! 1) список ТОЧНОГО вызова eth_simulateV1 для записи (tests/data/service_tax_2026-10-25/calls.json) собран этим же кодом
//!    (service_tax::calls) — тест сверяет файл с кодом; перезаписать: cargo test --test bsc_dex_service_tax -- --ignored
//!    write_service_tax_calls. Источник USDT — пара PancakeSwap V2 WBNB/USDT (getPair из
//!    tests/data/native_2026-10-25/arb_native_responses_2026-10-25.json), кошелёк — тот же, что в той записи.
//! 2) разбор ЖИВОГО ответа узла (responses.json: bsc-dataseed.bnbchain.org, блок 0x7bb0bc5, записал техлид рекордером
//!    tests/recorders/record_service_tax.js по calls.json коммита f16aa9e, sha256 сверен): налоги USDT 0 по всем трём
//!    переводам, газ перевода 51603, decimals 18.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::check::{service_tax, simulate}.

use alloy_primitives::{Address, U256, address};
use serde_json::{Value, json};

use arb::venues::bsc_dex::check::service_tax;
use arb::venues::bsc_dex::check::simulate::CallResult;

const DIR: &str = "tests/data/service_tax_2026-10-25/";
/// USDT BSC — как у BingX в сети BSC (tests/native_common).
const USDT: Address = address!("0x55d398326f99059fF775485246999027B3197955");
/// PancakeSwap V2 WBNB/USDT — ответ getPair в записи native_2026-10-25.
const SOURCE: Address = address!("0x16b9a82891338f9ba80e2d6970fdda79d1eb0dae");
/// Кошелёк записи native_2026-10-25.
const WALLET: Address = address!("0x8894e0a0c962cb723c1976a4421c95949be2d4e3");

/// Сумма пробы: 5·10^18 минимальных единиц (налог — доля, от знаков не зависит; знаки для разбора — ответ decimals()
/// в записи).
fn amount() -> U256 {
    U256::from(5u8) * U256::from(10u8).pow(U256::from(18u8))
}

/// Тело запроса eth_simulateV1 так же, как check/simulate.rs (блок подставит рекордер).
fn calls() -> Value {
    let list: Vec<Value> = service_tax::calls(USDT, SOURCE, WALLET, amount())
        .into_iter()
        .map(|c| json!({ "from": c.from, "to": c.to, "input": c.input }))
        .collect();
    json!({
        "about": "Проба переводов USDT (решение 67): eth_simulateV1, блок — последний на момент записи",
        "request": {
            "jsonrpc": "2.0", "id": 1, "method": "eth_simulateV1",
            "params": [{ "blockStateCalls": [{ "calls": list }], "validation": false }, "latest"]
        }
    })
}

#[test]
fn service_tax_calls_file_matches_code() {
    let have: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{DIR}calls.json")).unwrap())
            .unwrap();
    assert_eq!(
        have,
        calls(),
        "calls.json устарел: cargo test --test bsc_dex_service_tax -- --ignored write_service_tax_calls"
    );
}

#[test]
#[ignore = "вручную: пишет список вызова для записи ответа узла"]
fn write_service_tax_calls() {
    std::fs::write(
        format!("{DIR}calls.json"),
        serde_json::to_string_pretty(&calls()).unwrap() + "\n",
    )
    .unwrap();
}

#[test]
fn usdt_taxes_from_recorded_simulation() {
    let rec: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{DIR}responses.json")).unwrap())
            .unwrap();
    let r: Vec<CallResult> =
        serde_json::from_value(rec["response"]["result"][0]["calls"].clone()).unwrap();
    let hex = rec["decimals"]["result"]
        .as_str()
        .unwrap()
        .trim_start_matches("0x");
    let dec = u8::from_str_radix(hex.trim_start_matches('0'), 16).unwrap();
    let (t, gas) = service_tax::read(&r, amount(), dec).unwrap();
    assert_eq!(dec, 18);
    // Живой узел (блок 0x7bb0bc5): налога у USDT нет ни на одном из трёх переводов — измерено, а не предположено.
    assert!(
        t.buy_pct.is_zero() && t.sell_pct.is_zero() && t.transfer_pct.is_zero(),
        "{t:?}"
    );
    // Газ перевода на пустой адрес — вызов 7 записи.
    assert_eq!(gas, 51603);
}
