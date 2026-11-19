//! Файл: tests/bsc_dex_graph_noroute_gas.rs
//! Что делает: ревью решения 271 на сцене tests/route_common/fork.rs (реальные резервы записи routes_2026-10-29, метка
//! семейства v2_fork):
//! 1. Газ разрешений (quote/approve_gas.rs::units) у граф-маршрута, первый хоп которого — форк, без исполнителя у
//!    кошелька: настройка dex.universal_router у форка не нужна; читается ERC-20 allowance(кошелёк, роутер форка
//!    dex.router) на токене входа (покупка — USDT, продажа — монета), как делает бандл (exec/bundle.rs,
//!    wallet/approve.rs::fork_missing). Узел — локальный JSON-RPC по HTTP (адрес-петля, как
//!    tests/bsc_dex_tip_gas_plan.rs); ответ allowance с BSC не записан (узел из среды разработки закрыт), поэтому узел
//!    отвечает ошибкой JSON-RPC (как tests/bsc_dex_tip_gas_plan.rs на прочие методы), а тест сверяет сам запрос.
//! 2. Маршрут графа через промежуточную монету (USDT → WBNB → COIN, оба пула форка) идёт котировкой из новой ветки
//!    quote/routing.rs, только если промежуточная проверена и без налогов (Via::mid_ok — как exec/live.rs,
//!    exec/live_hops.rs); не проверена — прежняя ошибка классики NoRoute. В route_common у всех DEX один адрес
//!    роутера (0x…99), поэтому доля через WBNB-USDT pancakeswap_v2 + форк считается одним роутером (исполнима).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/route_common (fork.rs), arb::venues::bsc_dex::{quote::approve_gas, wallet::signer, node::abi}.

mod route_common;

use std::sync::Arc;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, address};
use arb::core::types::{Asset, CheckState, CoinCheck, Side, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::secrets::crypto::KdfParams;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::node::abi::IERC20;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::approve_gas;
use arb::venues::bsc_dex::wallet::signer;
use chrono::Utc;
use route_common::fork::{COIN, ROUTER, fork_pool, real_v2, routed, scene, setup, venue, zero};
use route_common::{USDT, WBNB};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

const KDF: KdfParams = KdfParams {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};
/// Пример EIP-155 (https://eips.ethereum.org/EIPS/eip-155, «Example») — тестовый ключ, как tests/bsc_dex_accounts.rs.
const KEY: &str = "0x4646464646464646464646464646464646464646464646464646464646464646";

/// Локальный узел: на любой запрос — ошибка JSON-RPC «method not found»; тела запросов копятся. Возвращает: адрес, тела.
async fn node() -> (String, Arc<Mutex<Vec<Value>>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let got = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let g = g.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];
                let k = s.read(&mut buf).await.unwrap_or(0);
                let text = String::from_utf8_lossy(&buf[..k]).to_string();
                let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
                let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
                let reply = json!({"jsonrpc": "2.0", "id": v["id"], "error": {"code": -32601, "message": "method not found"}}).to_string();
                g.lock().await.push(v);
                let r = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = s.write_all(r.as_bytes()).await;
            });
        }
    });
    (url, got)
}

/// Узел и кошелёк «тест» (строка «пробы») БЕЗ исполнителя: ключ примера EIP-155. Возвращает: адрес кошелька, тела.
async fn wallet_and_node(p: &Parts) -> (Address, Arc<Mutex<Vec<Value>>>) {
    let (url, got) = node().await;
    let g = |k: &str, v: Value| (k.to_string(), Scope::Global, v);
    for (k, sc, v) in [
        g("bsc.node_http_url", json!(url)),
        g("bsc.node_http_rps", json!(100)),
        g("http.timeout_ms", json!(5000)),
        g("http.body_idle_ms", json!(5000)),
        g("http.total_ms", json!(10000)),
        g("http.max_body_bytes", json!(1_000_000)),
    ] {
        p.ctx.settings.set(&k, sc, Some(v), "тест").await.unwrap();
    }
    let pw = || Zeroizing::new("pw".to_string());
    p.ctx.vault.create(pw(), KDF).await.unwrap();
    let key = Some(Zeroizing::new(KEY.into()));
    p.ctx
        .vault
        .change(pw(), "wallet.test.key", key, KDF)
        .await
        .unwrap();
    let a = p.ctx.settings.accounts();
    let id = a.reserve_id().await.unwrap();
    a.add(&id, "тест", "wallet.test.key", "bsc_dex", "тест")
        .await
        .unwrap();
    a.map_set(arb::settings::accounts::ROW_PROBE, Some(&id), "тест")
        .await
        .unwrap();
    (signer::address(&p.ctx).unwrap(), got)
}

/// Блокер ревью 271: первый хоп графа — форк, исполнителя нет — газ разрешений не требует dex.universal_router форка,
/// а читает allowance(кошелёк, dex.router форка) на токене входа (обе стороны).
#[tokio::test]
async fn fork_first_hop_approve_reads_fork_router() {
    let p = scene().await;
    venue(&p, "dex.router_mode", Some(json!("graph"))).await;
    let (wallet, got) = wallet_and_node(&p).await;
    for (side, token) in [(Side::Buy, USDT), (Side::Sell, COIN)] {
        let quote = routed(&p, side, false).await.expect("котировка графа");
        got.lock().await.clear();
        let r = approve_gas::units(&p.quoting(), &quote, 18, 18).await;
        if let Err(e) = &r {
            assert!(
                !e.message.contains("dex.universal_router"),
                "{side:?}: {}",
                e.message
            );
            assert_ne!(e.code, ErrorCode::NotConfigured, "{side:?}: {}", e.message);
        }
        let want = Bytes::from(
            IERC20::allowanceCall {
                owner: wallet,
                spender: ROUTER,
            }
            .abi_encode(),
        );
        let calls: Vec<Value> = got.lock().await.clone();
        assert!(
            calls.iter().any(|v| v["method"] == "eth_call"
                && v["params"][0]["to"]
                    .as_str()
                    .map(|s| s.parse::<Address>().ok())
                    == Some(Some(token))
                && v["params"][0]["input"] == json!(want)),
            "{side:?}: allowance(кошелёк, роутер форка) не запрошен: {calls:?}"
        );
    }
}

/// Ревью 271 (п. 2): путь графа USDT → WBNB → COIN по пулам форка; промежуточная WBNB не проверена — котировки нет
/// (ошибка классики NoRoute), проверена без налогов — исполнимая котировка графа из двух хопов.
#[tokio::test]
async fn graph_route_needs_checked_mid() {
    let p = setup().await;
    let r = real_v2(&p);
    fork_pool(
        &p,
        address!("0x00000000000000000000000000000000000000c3"),
        COIN,
        WBNB,
        r,
    );
    fork_pool(
        &p,
        address!("0x00000000000000000000000000000000000000c4"),
        WBNB,
        USDT,
        r,
    );
    venue(&p, "dex.router_mode", None).await;
    let classic = routed(&p, Side::Buy, false).await.expect_err("классика");
    assert_eq!(classic.code, ErrorCode::NoRoute, "{}", classic.message);
    venue(&p, "dex.router_mode", Some(json!("graph"))).await;
    let unchecked = routed(&p, Side::Buy, false)
        .await
        .expect_err("WBNB не проверена");
    assert_eq!(unchecked.code, ErrorCode::NoRoute);
    assert_eq!(unchecked.message, classic.message, "текст ошибки классики");
    p.checks.set_coin(CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            chain: None,
            ticker: "BNB".into(),
            contract: Some(WBNB),
        },
        taxes: CheckState::Known(zero()),
        source: "тест".into(),
        checked_at: Some(Utc::now()),
        wait: None,
    });
    let ok = routed(&p, Side::Buy, false).await.expect("WBNB проверена");
    let route = ok.route.expect("маршрут");
    assert!(route.executable, "{:?}", route.exec_note);
    assert!(
        route.legs.iter().all(|l| l.hops.len() == 2),
        "{:?}",
        route.legs
    );
}
