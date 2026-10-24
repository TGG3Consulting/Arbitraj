//! Файл: tests/bsc_dex_multicall_failover.rs
//! Что делает: Multicall3 и резервный узел на реальных данных.
//! 1) Вызов aggregate3 кодируется селектором, который есть в официальном байткоде Multicall3 (README mds1/multicall3:
//!    подписанная транзакция развёртывания по адресу 0xcA11…CA11; селектор 0x82ad56cb); ответ разбирается по вызовам:
//!    успех — адрес пула LIGHT/USDT PancakeSwap V3 (getPool, живые данные 2026-10-24), откат — записанный откат
//!    QuoterV2 «Unexpected error» с узла BSC (tests/bsc_dex_live_fixes.rs).
//! 2) Переключение на резервный узел: на ошибки «нет связи», таймаут, 5xx и 429 (текст 429 — из журнала живого
//!    запуска: «узел BSC (HTTP): превышен лимит запросов, HTTP 429»), не на «непонятный ответ» и не на ключ;
//!    порядок узлов с удержанием и без.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::node (multicall, failover), arb::venues::bsc_dex::errors. Моков нет.

use std::str::FromStr;

use alloy::sol_types::SolValue;
use alloy_primitives::{Address, Bytes};

use arb::venues::bsc_dex::errors::from_http;
use arb::venues::bsc_dex::node::failover::{Node, order, switchable};
use arb::venues::bsc_dex::node::multicall::{IMulticall3, decode, encode};

/// Селектор aggregate3((address,bool,bytes)[]) — из официального байткода Multicall3 (PUSH4 0x82ad56cb).
const AGGREGATE3: [u8; 4] = [0x82, 0xad, 0x56, 0xcb];
/// Откат QuoterV2 «Unexpected error», записанный с узла BSC (tests/bsc_dex_live_fixes.rs).
const UNEXPECTED: &str = "0x08c379a000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000010556e6578706563746564206572726f7200000000000000000000000000000000";

#[test]
fn aggregate3_encodes_and_splits_answers() {
    let factory = Address::from_str("0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865").unwrap();
    let data = encode(&[
        (factory, Bytes::from(vec![1, 2, 3])),
        (factory, Bytes::new()),
    ]);
    assert_eq!(&data[..4], &AGGREGATE3);
    // Ответ Multicall3: пул LIGHT/USDT (getPool → address) и откат второго вызова.
    let pool = Address::from_str("0xae02717e94c9b5bae817601a49b4584f58324015").unwrap();
    let revert = Bytes::from_str(UNEXPECTED).unwrap();
    let answer = vec![
        IMulticall3::Result {
            success: true,
            returnData: Bytes::from(pool.abi_encode()),
        },
        IMulticall3::Result {
            success: false,
            returnData: revert,
        },
    ]
    .abi_encode();
    let out = decode(&answer, 2).unwrap();
    assert_eq!(Address::abi_decode(out[0].as_ref().unwrap()).unwrap(), pool);
    assert!(out[1].as_ref().unwrap_err().contains("08c379a0"));
    // Число ответов не совпало с числом вызовов — ответ не понят, половине данных не верим.
    assert!(decode(&answer, 3).is_err());
}

#[test]
fn backup_takes_over_on_link_errors_only() {
    let limit = from_http(429, "узел BSC (HTTP)", "");
    assert_eq!(
        limit.message,
        "узел BSC (HTTP): превышен лимит запросов, HTTP 429"
    );
    assert!(switchable(&limit));
    assert!(switchable(&from_http(502, "узел BSC (HTTP)", "")));
    assert!(!switchable(&from_http(401, "узел BSC (HTTP)", "")));
    assert!(!switchable(&from_http(400, "узел BSC (HTTP)", "")));
    assert_eq!(order(false, true), vec![Node::Main]);
    assert_eq!(order(true, false), vec![Node::Main, Node::Backup]);
    assert_eq!(order(true, true), vec![Node::Backup, Node::Main]);
}
