//! Файл: tests/bybit_settlement.rs
//! Что делает: расчётный круг Base у Bybit (venues/bybit/settlement.rs) на ЖИВОМ записанном ответе instruments-info
//! спота от 2026-11-04 (tests/data/cex_samples_2026_11_04/bybit_instruments_spot.json): символ каждой пары —
//! baseCoin + quoteCoin, в т.ч. USDCUSDT — на этом факте стоит сверка сохранённой заявки (matches_symbol).
//! Приватные ответы (ордер, вывод, депозит) — тест #[ignore] внизу: что записать на живом боте.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bybit::settlement. Моков нет.

use serde_json::Value;

use arb::venues::bybit::settlement::matches_symbol;

const INSTRUMENTS: &str = "tests/data/cex_samples_2026_11_04/bybit_instruments_spot.json";

/// Все пары записанного справочника: symbol = baseCoin + quoteCoin; чужая котируемая монета и перестановка — не
/// совпадают; USDCUSDT есть и торгуется.
#[test]
fn settlement_symbol_matches_live_instruments() {
    let t = std::fs::read_to_string(INSTRUMENTS).expect("запись");
    let v: Value = serde_json::from_str(&t).expect("JSON");
    assert_eq!(v["retCode"], 0);
    let list = v["result"]["list"].as_array().expect("list");
    assert!(list.len() > 300, "пар: {}", list.len());
    for s in list {
        let (sym, base, quote) = (
            s["symbol"].as_str().expect("symbol"),
            s["baseCoin"].as_str().expect("baseCoin"),
            s["quoteCoin"].as_str().expect("quoteCoin"),
        );
        assert!(matches_symbol(Some(sym), base, quote), "{sym}");
        assert!(!matches_symbol(Some(sym), quote, base), "{sym}");
    }
    let usdc = list
        .iter()
        .find(|s| s["symbol"] == "USDCUSDT")
        .expect("USDCUSDT в записи");
    assert_eq!(usdc["status"], "Trading");
    assert!(matches_symbol(Some("USDCUSDT"), "USDC", "USDT"));
    assert!(!matches_symbol(Some("USDCUSDT"), "USDC", "USDC"));
    assert!(!matches_symbol(None, "USDC", "USDT"));
    assert!(!matches_symbol(Some("USDCUSDT"), "", "USDCUSDT"));
}

/// Записать на живом боте (ключ Bybit): (1) IOC USDCUSDT — ответ POST /v5/order/create и GET /v5/order/realtime?
/// category=spot&orderLinkId= (окончательная запись: orderStatus, qty, side, cumExecQty, cumExecValue, cumFeeDetail);
/// (2) вывод USDC по BASE с feeType=1 — GET /v5/asset/withdraw/query-record?withdrawID= (что в amount: введённая сумма
/// или за вычетом withdrawFee) и повтор POST /v5/asset/withdraw/create с тем же requestId (ответ на повтор);
/// (3) депозит USDC по BASE при депозите на UNIFIED — GET /v5/asset/deposit/query-record?coin=USDC&txID= (status) и
/// GET /v5/account/transaction-log?accountType=UNIFIED&coin=USDC (запись TRANSFER_IN и её время).
#[test]
#[ignore = "нет записей ордера USDCUSDT, вывода и депозита USDC по BASE — записать на живом боте"]
fn settlement_private_live() {}
