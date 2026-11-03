//! Файл: tests/bsc_dex_probe_paths.rs
//! Что делает: выбор пути проб у пулов без своего Universal Router (решение 122 X; заметка
//! docs/notes/probe-executor-2026-11-03.md):
//! - проба налога через пул вида без своего пути (check/probe_exec.rs::no_path): шаг исполнителя у вида есть, а
//!   exec.executor_address не задан — «не задана настройка» с её именем; вида шага нет — не настройка (NoRoute);
//! - откат исполнителя `require(…, "deadline")` — «срок вышел» (помеха), а не вердикт по монете (probe_calls::late);
//! - проба нативной монеты (exec/native.rs::trade_path): у PancakeSwap / Uniswap (свой Universal Router) путь BNB есть;
//!   у форка V2 (свой classic V2 Router) и у вида только с исполнителем сделки BNB нет — причина, без требования
//!   dex.universal_router.
//!
//! Пул — из реальной записи CAKE (tests/route_common, routes_2026-10-29); вид пула подменяется видом DEX из каталога
//! (pools/families.rs::make) — проверяется только выбор пути по виду, без сети.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{check::{probe_exec, probe_calls, simulate}, exec::native, pools}, tests/route_common.

mod route_common;

use alloy::sol_types::{Revert, SolError};
use alloy_primitives::{Bytes, U64};

use arb::core::types::PoolRef;
use arb::gateway::errors::ErrorCode;
use arb::venues::bsc_dex::check::probe_calls;
use arb::venues::bsc_dex::check::probe_exec;
use arb::venues::bsc_dex::check::simulate::CallResult;
use arb::venues::bsc_dex::exec::native;
use arb::venues::bsc_dex::pools::families::make;

use route_common::CAKE_USDT_PCS;

#[tokio::test]
async fn executor_kind_without_address_is_setting_and_no_kind_is_not() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let mut e = q
        .reg
        .get(&PoolRef::Contract(CAKE_USDT_PCS))
        .expect("пул в реестре");
    // THENA V1 (Solidly): шаг SOLIDLY есть, адрес исполнителя в тесте не задан — настройка exec.executor_address.
    e.family = make("thena", "solidly_v1").expect("вид из каталога");
    let err = probe_exec::no_path(&q.ctx, &e);
    assert_eq!(err.code, ErrorCode::NotConfigured);
    assert_eq!(err.setting.as_deref(), Some("exec.executor_address"));
    // KyberSwap Elastic: шага исполнителя нет — не настройка, «не проверено» с причиной.
    e.family = make("kyberswap_elastic", "kyber_elastic").expect("вид из каталога");
    let err = probe_exec::no_path(&q.ctx, &e);
    assert_eq!(err.code, ErrorCode::NoRoute);
    assert_eq!(err.setting, None);
}

/// Итог вызова симуляции с откатом и данными отката. Ошибок нет.
fn reverted(data: Vec<u8>) -> CallResult {
    serde_json::from_value(serde_json::json!({
        "status": format!("{:#x}", U64::ZERO),
        "returnData": Bytes::from(data),
        "gasUsed": "0x0",
        "logs": [],
        "error": {"code": 3, "message": "execution reverted"}
    }))
    .expect("итог вызова по схеме execution-apis")
}

#[test]
fn executor_deadline_revert_is_late() {
    let late = Revert {
        reason: "deadline".into(),
    };
    assert!(probe_calls::late(&reverted(late.abi_encode())));
    let min = Revert {
        reason: "minOut".into(),
    };
    assert!(!probe_calls::late(&reverted(min.abi_encode())));
}

#[test]
fn native_probe_only_where_bnb_trade_exists() {
    for (id, kind) in [
        ("pancakeswap", "pancakeswap_v2"),
        ("pancakeswap", "pancakeswap_v3"),
        ("uniswap", "uniswap_v2"),
        ("uniswap", "uniswap_v3"),
    ] {
        let f = make(id, kind).expect("вид из каталога");
        assert!(native::trade_path(f).is_ok(), "{id} {kind}");
    }
    for (id, kind) in [
        ("nomiswap", "v2_fork"),
        ("biswap", "v2_fork"),
        ("thena", "solidly_v1"),
        ("topaz", "slipstream"),
        ("some_v3", "v3_fork"),
    ] {
        let f = make(id, kind).expect("вид из каталога");
        let why = native::trade_path(f).expect_err(kind);
        assert!(!why.contains("dex.universal_router"), "{why}");
        assert!(why.contains("сделки BNB через этот пул нет"), "{why}");
    }
}
