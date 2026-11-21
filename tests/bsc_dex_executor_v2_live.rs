//! Файл: tests/bsc_dex_executor_v2_live.rs
//! Что делает: решение 272 — разбор факта executeNative по НАСТОЯЩИМ квитанциям сети BSC. Пока исполнитель v2 не
//! развёрнут, записи нет — тест помечен #[ignore] и при запуске без записи падает с инструкцией.
//! Как снять запись (после переразвёртывания, docs/notes/executor-v2-redeploy.md):
//!   1. Сделать по одной маленькой сделке BNB через исполнитель v2: продажу и покупку (через пул форка — так BNB идёт
//!      только исполнителем). Хеши транзакций — в хронологии сделки (exec/swap_ctx.rs «чем найти»).
//!   2. Квитанции своим узлом: `eth_getTransactionReceipt` (execution-apis src/eth/transaction.yaml) — ответ `result`
//!      как есть в tests/data/executor_v2_live/receipt_sell.json и receipt_buy.json.
//!   3. Рядом meta.json: {"executor": "0x…", "wallet": "0x…", "usdt": "0x…", "sell": {"bnb": "<wei продано>",
//!      "usdt": "<ед. USDT получено>"}, "buy": {"bnb": "<wei куплено>", "usdt": "<ед. USDT потрачено>"}} — числа из
//!      хронологии сделки и BscScan.
//!   4. `cargo test --test bsc_dex_executor_v2_live -- --ignored`.
//!
//! Кто вызывает: cargo test -- --ignored.
//! От чего зависит: arb::venues::bsc_dex::{exec::{executor::fact, fill}, send::track::Receipt}.

use alloy_primitives::{Address, U256};
use serde_json::Value;

use arb::venues::bsc_dex::exec::executor::fact;
use arb::venues::bsc_dex::exec::fill;
use arb::venues::bsc_dex::send::track::Receipt;

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/executor_v2_live");

fn read(name: &str) -> Value {
    let path = format!("{DIR}/{name}");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!("нет записи {path} — как снять, шапка tests/bsc_dex_executor_v2_live.rs")
    });
    serde_json::from_str(&text).unwrap()
}

#[test]
#[ignore = "запись квитанций executeNative снимается после развёртывания исполнителя v2"]
fn native_fact_from_live_receipts() {
    let meta = read("meta.json");
    let a = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let u = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let (x, w, usdt) = (a(&meta["executor"]), a(&meta["wallet"]), a(&meta["usdt"]));
    let sell: Receipt = serde_json::from_value(read("receipt_sell.json")).unwrap();
    let (n_in, n_out) = fact::settled(&sell, x).expect("NativeSettled в квитанции продажи");
    assert_eq!(n_in - n_out, u(&meta["sell"]["bnb"]));
    assert_eq!(
        fill::moved(&sell, usdt, None, Some(w)),
        u(&meta["sell"]["usdt"])
    );
    let buy: Receipt = serde_json::from_value(read("receipt_buy.json")).unwrap();
    let (b_in, b_out) = fact::settled(&buy, x).expect("NativeSettled в квитанции покупки");
    assert_eq!((b_in, b_out), (U256::ZERO, u(&meta["buy"]["bnb"])));
    let back = fill::moved(&buy, usdt, Some(x), Some(w));
    assert_eq!(
        fill::moved(&buy, usdt, Some(w), None) - back,
        u(&meta["buy"]["usdt"])
    );
}
