//! Файл: tests/bsc_dex_executor_v2.rs
//! Что делает: решение 272 — исполнитель v2 (executeNative, нативный BNB монетой сделки) без сети и без моков:
//! - версия по коду в сети: код v2 (contracts/out/) и v1 (contracts/out/v1/, развёрнутые до решения 272) узнаются,
//!   чужой — нет; хеш кода v1 — тот, что был в репозитории до решения (коммит 2180cf9);
//! - execute не изменился: подпись, селектор и запись ABI у v1 и v2 одинаковы; в v2 есть executeNative и NativeSettled;
//!   обёртка (ревью 272 А1) — аргумент конструктора v2, геттер wnative(), слот 1;
//! - вызов: with_native — данные executeNativeCall, value = вход у продажи и 0 у покупки, край маршрута не обёртка —
//!   отказ; без режима — execute, как раньше;
//! - факт: событие NativeSettled и Transfer USDT по ЗАПИСИ исполнения настоящего байткода v2 на EVM ethereumjs
//!   (tests/data/executor_v2_evm_2026-11-14, contracts/evmtest/run.mjs; лога из сети BSC до развёртывания v2 нет);
//! - котировка: исполнитель v1 (версия не сверена) — нативный BNB через вид «только исполнитель» неисполним с тем же
//!   текстом, что до решения; v2 — исполним; фильтр стороны BNB (exec/native_exec.rs) при v1 = native::pool_ok.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{exec::{executor, native, native_exec}, quote, pools}, tests/route_common.

mod route_common;

use std::collections::{HashMap, HashSet};

use alloy::json_abi::JsonAbi;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256, address};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::{PoolKind, PoolRef, Side};
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::chain::Step;
use arb::venues::bsc_dex::exec::executor::abi::ArbExecutor;
use arb::venues::bsc_dex::exec::executor::code::{self, Version};
use arb::venues::bsc_dex::exec::executor::{self, call, fact};
use arb::venues::bsc_dex::exec::fill;
use arb::venues::bsc_dex::exec::native_exec;
use arb::venues::bsc_dex::exec::plan::Order;
use arb::venues::bsc_dex::pools::{families, graph};
use arb::venues::bsc_dex::quote::path::Path;
use arb::venues::bsc_dex::quote::{graph_exec, graph_legs, route_calc, route_paths};
use arb::venues::bsc_dex::send::track::Receipt;

use route_common::{BTCB, CAKE, USDT, WBNB, WBNB_USDT_V2};

const EXECUTOR: &str = "0x00000000000000000000000000000000000000ee";
const EXEC: Address = address!("0x00000000000000000000000000000000000000ee");

fn abi(path: &str) -> JsonAbi {
    let text = std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn version_by_code_in_chain() {
    // Решение 395 (t181): v2 — замороженная копия contracts/out/v2/; решение 457: текущая сборка contracts/out/ — v4,
    // v3 — замороженная копия contracts/out/v3/.
    let v4 = code::runtime().expect("код v4 сверен");
    let v3 = code::runtime_v3().expect("код v3 сверен по out/v3/ArbExecutor.sha256");
    let v2 = code::runtime_v2().expect("код v2 сверен по out/v2/ArbExecutor.sha256");
    let v1 = code::runtime_v1().expect("код v1 сверен по out/v1/ArbExecutor.sha256");
    assert_ne!(v1, v2);
    assert_ne!(v2, v3);
    assert_ne!(v3, v4);
    assert_eq!(code::version_of(&v4).unwrap(), Some(Version::V4));
    assert_eq!(code::current_version().unwrap(), Version::V4);
    assert_eq!(code::version_of(&v3).unwrap(), Some(Version::V3));
    assert_eq!(code::version_of(&v2).unwrap(), Some(Version::V2));
    assert_eq!(code::version_of(&v1).unwrap(), Some(Version::V1));
    assert_eq!(
        code::version_of(&Bytes::from(vec![0x60, 0x00])).unwrap(),
        None
    );
    // Хеш кода v1 — записанный в репозитории до решения 272 (contracts/out/ArbExecutor.sha256 коммита 2180cf9).
    let sha = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/contracts/out/v1/ArbExecutor.sha256"
    ))
    .unwrap();
    assert!(sha.contains(
        "5ed9134e2348356468470c5a2fb46769b67fd122a2240b430a149ae9d8edc5ba  ArbExecutor.bin-runtime"
    ));
    assert_eq!(Version::parse("v2"), Some(Version::V2));
    assert_eq!(Version::parse(Version::V1.label()), Some(Version::V1));
    assert_eq!(Version::parse(""), None);
}

#[test]
fn execute_unchanged_and_execute_native_added() {
    let (v1, v2) = (
        abi("contracts/out/v1/ArbExecutor.abi"),
        abi("contracts/out/ArbExecutor.abi"),
    );
    let (f1, f2) = (
        &v1.function("execute").unwrap()[0],
        &v2.function("execute").unwrap()[0],
    );
    assert_eq!(f1, f2, "запись ABI execute у v1 и v2 одна и та же");
    assert_eq!(f2.selector(), ArbExecutor::executeCall::SELECTOR);
    assert!(v1.function("executeNative").is_none());
    let n = &v2.function("executeNative").expect("executeNative в v2")[0];
    assert_eq!(n.selector(), ArbExecutor::executeNativeCall::SELECTOR);
    assert_eq!(
        n.signature(),
        "executeNative(bool,address,uint256,address,uint256,uint256,(uint8,address,address,address,uint16,bytes)[])"
    );
    // Ревью 272 (А1): обёртка — аргумент конструктора v2 и геттер wnative(); у v1 конструктор без аргументов.
    let ctor = |a: &JsonAbi| a.constructor.as_ref().map(|c| c.inputs.len());
    assert_eq!((ctor(&v1), ctor(&v2)), (Some(0), Some(1)));
    assert!(v2.function("wnative").is_some() && v1.function("wnative").is_none());
    assert_eq!(
        code::wnative_slot().unwrap(),
        alloy_primitives::B256::from(U256::from(1u8))
    );
    assert_eq!(
        n.state_mutability,
        alloy::json_abi::StateMutability::Payable
    );
    assert!(v2.events.contains_key("NativeSettled") && !v1.events.contains_key("NativeSettled"));
}

/// Шаг одной доли через пул записи WBNB/USDT (PancakeSwap V2).
fn step(
    q: &arb::venues::bsc_dex::quote::Quoting,
    (tin, tout): (Address, Address),
    amount: U256,
) -> Step {
    Step {
        entry: q.reg.get(&PoolRef::Contract(WBNB_USDT_V2)).expect("пул"),
        order: Order {
            token_in: tin,
            token_out: tout,
            amount_in: amount,
            min_out: U256::from(5u8),
        },
        from_wallet: true,
        to_wallet: true,
    }
}

#[tokio::test]
async fn native_call_encoding_and_value() {
    let p = route_common::parts().await;
    let v = Scope::Venue("bsc_dex".into());
    p.ctx
        .settings
        .set("dex.deadline_s", v, Some(json!(60)), "тест")
        .await
        .unwrap();
    let q = p.quoting();
    let amt = U256::from(10u64).pow(U256::from(18u8));
    let sell = call::build(&q.ctx, &[vec![step(&q, (WBNB, USDT), amt)]], EXEC, 1_000).unwrap();
    assert!(sell.native.is_none() && sell.value().is_zero());
    assert_eq!(
        &sell.data()[..4],
        ArbExecutor::executeCall::SELECTOR.as_slice(),
        "без режима — execute"
    );
    let n = sell
        .clone()
        .with_native(&q.ctx, WBNB, true)
        .expect("продажа: вход — обёртка");
    assert_eq!(n.value(), amt, "value = вход");
    let d = ArbExecutor::executeNativeCall::abi_decode(&n.data()).expect("executeNative");
    assert_eq!(
        (d.wrapIn, d.tokenIn, d.amountIn, d.tokenOut),
        (true, WBNB, amt, USDT)
    );
    assert_eq!(
        (d.minOut, d.deadline, d.steps.len()),
        (U256::from(5u8), U256::from(1_060u64), 1)
    );
    let e = sell
        .with_native(&q.ctx, WBNB, false)
        .expect_err("продажа с разворачиванием выхода");
    assert_eq!(e.code, ErrorCode::NoRoute);
    let buy = call::build(&q.ctx, &[vec![step(&q, (USDT, WBNB), amt)]], EXEC, 1_000).unwrap();
    let b = buy
        .with_native(&q.ctx, WBNB, false)
        .expect("покупка: выход — обёртка");
    assert!(b.value().is_zero());
    assert!(
        !ArbExecutor::executeNativeCall::abi_decode(&b.data())
            .unwrap()
            .wrapIn
    );
}

/// Квитанция из записи EVM: адрес, темы и данные событий — из записи; поля блока и транзакции, которых у EVM без
/// блока нет, — пустые (null), как у ожидающей транзакции в eth_getTransactionReceipt (execution-apis Log).
fn receipt(case: &Value) -> Receipt {
    let logs: Vec<Value> = case["logs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            json!({
                "address": l["address"], "topics": l["topics"], "data": l["data"], "blockHash": null,
                "blockNumber": null, "transactionHash": null, "transactionIndex": null, "logIndex": null,
                "removed": false
            })
        })
        .collect();
    serde_json::from_value(json!({
        "status": "0x1", "blockNumber": "0x1", "gasUsed": "0x1", "effectiveGasPrice": "0x1", "logs": logs
    }))
    .unwrap()
}

#[test]
fn native_settled_from_recorded_evm_logs() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/executor_v2_evm_2026-11-14/native_settled.json"
    ))
    .unwrap();
    let rec: Value = serde_json::from_str(&text).unwrap();
    let c = &rec["cases"];
    let a = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let u = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let s = &c["sell"];
    let r = receipt(s);
    assert_eq!(
        fact::settled(&r, a(&s["executor"])),
        Some((u(&s["value"]), U256::ZERO))
    );
    assert_eq!(
        fill::moved(&r, a(&s["usdt"]), None, Some(a(&s["owner"]))),
        u(&s["usdt_got"])
    );
    assert_eq!(
        fact::settled(&r, a(&s["owner"])),
        None,
        "событие — только исполнителя"
    );
    let l = &c["sell_left"];
    assert_eq!(
        fact::settled(&receipt(l), a(&l["executor"])),
        Some((u(&l["value"]), u(&l["returned"])))
    );
    let b = &c["buy"];
    let r = receipt(b);
    assert_eq!(
        fact::settled(&r, a(&b["executor"])),
        Some((U256::ZERO, u(&b["bnb_got"])))
    );
    assert_eq!(
        fill::moved(&r, a(&b["usdt"]), Some(a(&b["owner"])), None),
        u(&b["usdt_spent"])
    );
}

/// Путь записи глубины 3 (как tests/bsc_dex_multihop.rs three_hop), укороченный до одного хопа вида `kind`.
fn one_hop(q: &arb::venues::bsc_dex::quote::Quoting, kind: &str) -> Path {
    let g = graph::build(&q.ctx, &q.reg, &q.tokens, 18).expect("граф");
    let allowed: HashSet<Address> = [WBNB, BTCB].into_iter().collect();
    let paths = route_paths::enumerate(&g, USDT, CAKE, &allowed, 3, 50);
    let three = paths
        .iter()
        .find(|p| p.hops() == 3)
        .expect("путь глубины 3");
    let dec: HashMap<Address, u8> = [CAKE, USDT, WBNB, BTCB]
        .into_iter()
        .map(|t| (t, 18))
        .collect();
    let legs = graph_legs::legs_for(q, three, Side::Buy, 18, &dec).expect("ноги");
    let mut path =
        route_calc::eval(&q.ctx, &legs, Side::Buy, Decimal::ONE, Decimal::ONE).expect("путь");
    path.hops.truncate(1);
    path.hops[0].entry.family = families::make("x-272", kind).unwrap();
    path
}

#[tokio::test]
async fn quote_gates_follow_executor_version() {
    let p = route_common::parts().await;
    let q = p.quoting();
    route_common::executor(&p, EXECUTOR).await;
    let slip = one_hop(&q, "slipstream");
    let solidly = families::make("x-272-sol", "solidly_v1").unwrap();
    let infinity = families::make("x-272-inf", "pancakeswap_infinity_cl").unwrap();
    // v1 (версия не сверена) — как до решения 272.
    assert!(!executor::native_capable(&q.ctx));
    let why = graph_exec::executable(&q.ctx, std::slice::from_ref(&slip), true).expect_err("v1");
    assert!(why.contains("без своего пути исполнения"), "{why}");
    graph_exec::executable(&q.ctx, std::slice::from_ref(&slip), false)
        .expect("ненативная монета — как раньше");
    assert!(!native_exec::edge_ok(&q.ctx, PoolKind::Solidly, solidly));
    assert!(
        native_exec::edge_ok(&q.ctx, PoolKind::V2, solidly),
        "V2/V3 — как native::pool_ok"
    );
    // Сверенная версия v2 у кошелька, обёртка в исполнителе = dex.native_wrapped (ревью 272 А1).
    let a = p.ctx.settings.accounts();
    let w = a.wallets().into_iter().find(|w| w.name == "тест").unwrap();
    let wb = Some(format!("{WBNB:#x}"));
    a.set_executor_version(&w.id, EXECUTOR, Some("v2".into()), wb.clone(), "тест")
        .await
        .unwrap();
    assert!(
        !executor::native_capable(&q.ctx),
        "dex.native_wrapped не задана — нет"
    );
    let v = Scope::Venue("bsc_dex".into());
    p.ctx
        .settings
        .set(
            "dex.native_wrapped",
            v,
            Some(json!(WBNB.to_string())),
            "тест",
        )
        .await
        .unwrap();
    assert!(executor::native_capable(&q.ctx));
    graph_exec::executable(&q.ctx, std::slice::from_ref(&slip), true)
        .expect("v2 — BNB через исполнитель");
    assert!(native_exec::edge_ok(&q.ctx, PoolKind::Solidly, solidly));
    assert!(
        !native_exec::edge_ok(&q.ctx, PoolKind::Cl, infinity),
        "Infinity на стороне BNB — шаг пула менеджера не берётся (решение 395: ExecKind::singleton)"
    );
    // Смена адреса — версия сброшена (не сверена = v1).
    a.set_executor(
        &w.id,
        Some("0x00000000000000000000000000000000000000ef".into()),
        "тест",
    )
    .await
    .unwrap();
    assert!(!executor::native_capable(&q.ctx));
    // Версия записывается только для того адреса, что сверяли.
    a.set_executor_version(&w.id, EXECUTOR, Some("v2".into()), wb, "тест")
        .await
        .unwrap();
    assert!(!executor::native_capable(&q.ctx));
}
