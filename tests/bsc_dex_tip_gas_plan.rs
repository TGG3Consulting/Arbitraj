//! Файл: tests/bsc_dex_tip_gas_plan.rs
//! Что делает: газ перевода чаевых канала в ПЛАНЕ (Тигран 09.11: согласовано; send/tip_gas.rs) на настоящем адаптере
//! DEX (окружение route_common на настоящей SQLite, настоящие секреты, кошелёк «тест» строки «пробы») и локальном
//! JSON-RPC узле по HTTP (адрес-петля разрешён для bsc.node_http_url — tests/settings_url_loopback.rs). Моков логики нет.
//! Ответ узла на eth_estimateGas — 0x5208 = 21000: gasUsed НАСТОЯЩЕЙ квитанции простого перевода BNB (тот же вид
//! транзакции, что перевод чаевых: BNB на адрес, пустые данные) — tests/data/receipts_2026-10-25/responses.json, блок
//! 129702236, tx 0xb306…8b42 (как tests/bsc_dex_tip_gas_236.rs); сам ответ eth_estimateGas не записан (узел BSC из
//! среды разработки закрыт), вид ответа — JSON-RPC 2.0 result-строка QUANTITY (execution-apis execute.yaml, eth_estimateGas).
//! Проверяется:
//! - planned_bnb: запрос узлу — from = кошелёк, to = channel.tip_address, value = channel.tip_bnb в wei, пустые данные
//!   (как exec/txs.rs при отправке); газ = 21000 × цена газа; второй раз — из памяти, без запроса;
//! - чаевые выключены (channel.tip_enabled = нет) — газа нет и запроса нет;
//! - net_check цикла DEX ↔ DEX вычитает и газ перевода чаевых: (газ + чаевые + газ перевода) × курс.
//! Кто вызывает: cargo test.
//! От чего зависит: route_common, arb::venues::bsc_dex::{send::tip_gas, exec::cycle_steps, quote::cycle_cost}.

mod route_common;

use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use alloy_primitives::{U256, address};
use arb::secrets::crypto::KdfParams;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::cycle_steps;
use arb::venues::bsc_dex::quote::cycle_cost::Rates;
use arb::venues::bsc_dex::send::tip_gas;
use arb::venues::bsc_dex::wallet::signer;
use route_common::{channel, executor, parts};
use rust_decimal::Decimal;
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
/// Записанный адрес исполнителя из решения 132 (tests/core_wallet_mapping.rs) — нужен помощнику кошелька «тест».
const EXECUTOR: &str = "0x9ef558a5d437718dde44e00990e0efb9976cf794";

/// Локальный узел: на eth_estimateGas — 0x5208 (шапка). Возвращает: адрес, счётчик запросов, последнее тело.
async fn node() -> (String, Arc<AtomicUsize>, Arc<Mutex<Value>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let n = Arc::new(AtomicUsize::new(0));
    let last = Arc::new(Mutex::new(Value::Null));
    let (c, b) = (n.clone(), last.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let (c, b) = (c.clone(), b.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];
                let k = s.read(&mut buf).await.unwrap_or(0);
                let text = String::from_utf8_lossy(&buf[..k]).to_string();
                let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
                let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
                let reply = if v["method"] == "eth_estimateGas" {
                    c.fetch_add(1, Ordering::SeqCst);
                    *b.lock().await = v.clone();
                    json!({"jsonrpc": "2.0", "id": v["id"], "result": "0x5208"})
                } else {
                    json!({"jsonrpc": "2.0", "id": v["id"], "error": {"code": -32601, "message": "method not found"}})
                }
                .to_string();
                let r = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = s.write_all(r.as_bytes()).await;
            });
        }
    });
    (url, n, last)
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

#[tokio::test]
async fn tip_transfer_gas_in_plan() {
    let p = parts().await;
    let (url, n, last) = node().await;
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
    // Кошелёк «тест» (строка «пробы») с ключом примера EIP-155.
    let pw = || Zeroizing::new("pw".to_string());
    p.ctx.vault.create(pw(), KDF).await.unwrap();
    p.ctx
        .vault
        .change(
            pw(),
            "wallet.test.key",
            Some(Zeroizing::new(KEY.into())),
            KDF,
        )
        .await
        .unwrap();
    executor(&p, EXECUTOR).await;
    let wallet = signer::address(&p.ctx).unwrap();
    // Один готовый канал бандлом с чаевыми (send::tip — только так чаевые и платятся).
    channel(&p, "club48", "bundle").await;
    let tip_to = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
    let ch = Scope::Channel("club48".into());
    for (k, v) in [
        ("channel.tip_enabled", json!(true)),
        ("channel.tip_bnb", json!("0.0001")),
        ("channel.tip_address", json!(tip_to.to_string())),
    ] {
        p.ctx
            .settings
            .set(k, ch.clone(), Some(v), "тест")
            .await
            .unwrap();
    }
    // Цена газа квитанции: effectiveGasPrice 0x2faf081 = 50 000 001 wei.
    let price = U256::from(50_000_001u64);
    let got = tip_gas::planned_bnb(&p.ctx, price).await.unwrap();
    assert_eq!(
        got,
        Some(dec("0.000001050000021")),
        "21000 × 50 000 001 wei"
    );
    assert_eq!(n.load(Ordering::SeqCst), 1);
    let body = last.lock().await.clone();
    let tx = &body["params"][0];
    assert_eq!(
        tx["from"].as_str().unwrap().to_lowercase(),
        wallet.to_string().to_lowercase()
    );
    assert_eq!(
        tx["to"].as_str().unwrap().to_lowercase(),
        tip_to.to_string().to_lowercase()
    );
    // 0.0001 BNB = 10^14 wei.
    assert_eq!(tx["value"], json!("0x5af3107a4000"));
    assert_eq!(tx["input"], json!("0x"));
    // Второй раз — из памяти.
    assert_eq!(tip_gas::planned_units(&p.ctx).await.unwrap(), Some(21000));
    assert_eq!(n.load(Ordering::SeqCst), 1, "оценка запомнена");
    // Цикл: газ 300 000 × 1 gwei = 0,0003 BNB, чаевые 0,0001, газ перевода 21000 × 1 gwei = 0,000021 BNB;
    // × 600 = 0,2526 базовой; чистая 101 − 100 − 0,2526 = 0,7474.
    p.ctx
        .settings
        .set(
            "dex_dex.min_profit_usd",
            Scope::Venue("bsc_dex".into()),
            Some(json!("0.5")),
            "тест",
        )
        .await
        .unwrap();
    let r = Rates {
        base_usdt: Decimal::ONE,
        bnb_usdt: Decimal::new(600, 0),
        bnb_in_base: Decimal::new(600, 0),
    };
    let gwei = U256::from(1_000_000_000u64);
    let net = cycle_steps::net_check(&p.ctx, (dec("101"), dec("100")), (300_000, gwei), &r)
        .await
        .unwrap();
    assert_eq!(net, dec("0.7474"));
    // Чаевые выключены — газа перевода нет, узел не спрашивается.
    p.ctx
        .settings
        .set(
            "channel.tip_enabled",
            ch.clone(),
            Some(json!(false)),
            "тест",
        )
        .await
        .unwrap();
    assert_eq!(tip_gas::planned_bnb(&p.ctx, price).await.unwrap(), None);
    let net = cycle_steps::net_check(&p.ctx, (dec("101"), dec("100")), (300_000, gwei), &r)
        .await
        .unwrap();
    assert_eq!(net, dec("0.82"));
    assert_eq!(n.load(Ordering::SeqCst), 1);
}
