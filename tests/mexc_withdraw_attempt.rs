//! NXT 26.11: сравнение записи журнала с фактическими полями POST.
//! Тестовые ключи и ответы принадлежат локальному HTTP-серверу.
mod usdt_return_common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arb::core::types::{Asset, VenueId, WithdrawRequest};
use arb::gateway::errors::ErrorCode;
use arb::secrets::{Vault, crypto::KdfParams};
use arb::settings::scope::Scope;
use arb::storage::raw_log::RawLog;
use arb::venues::mexc::{ctx::Ctx, transfers};
use axum::{
    Json, Router,
    extract::Query,
    routing::{get, post},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use zeroize::Zeroizing;

#[tokio::test]
async fn withdrawal_attempt_matches_wire_without_exposing_address_or_credentials() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let capture = requests.clone();
    let histories = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reads = histories.clone();
    let app = Router::new()
        .route(
            "/api/v3/capital/withdraw",
            post(move |Query(q): Query<HashMap<String, String>>| {
                let capture = capture.clone();
                async move {
                    let n = {
                        let mut seen = capture.lock().unwrap();
                        seen.push(q);
                        seen.len()
                    };
                    if n == 4 {
                        // Соединение принято; отсутствие ответа должно запустить только сверку истории.
                        tokio::time::sleep(Duration::from_secs(4)).await;
                    }
                    if n == 1 {
                        Json(json!({"code":10212, "msg":"Withdrawal failed"}))
                    } else {
                        Json(json!({"id":"fixture-mexc-id"}))
                    }
                }
            }),
        )
        .route(
            "/api/v3/capital/withdraw/history",
            get(move || {
                reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { Json(json!([])) }
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = usdt_return_common::dir("mexc-withdraw-attempt");
    let venue = Scope::Venue("mexc".into());
    let (core, journal) = usdt_return_common::start_with(
        &dir,
        None,
        vec![
            ("http.timeout_ms", Scope::Global, json!(1000)),
            ("http.body_idle_ms", Scope::Global, json!(1000)),
            ("http.total_ms", Scope::Global, json!(1500)),
            ("http.max_body_bytes", Scope::Global, json!(1_000_000)),
            ("venue.rest_url", venue.clone(), json!(url)),
            ("venue.recv_window_ms", venue, json!(5000)),
        ],
    )
    .await;
    let settings = core.inner.deps.settings.clone();
    let vault = Vault::locked(dir.join("mexc.enc"));
    let kdf = KdfParams {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };
    let pw = || Zeroizing::new("local fixture".to_owned());
    vault.create(pw(), kdf).await.unwrap();
    for name in ["mexc.api_key", "mexc.api_secret"] {
        vault
            .change(
                pw(),
                name,
                Some(Zeroizing::new(format!("fixture-{name}"))),
                kdf,
            )
            .await
            .unwrap();
    }
    let ctx = Ctx {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault,
        raw: RawLog::start(dir.join("raw"), settings),
        journal: journal.clone(),
        taps: Default::default(),
    };
    let address = "0xABcdef0000000000000000000000000000000000";
    let lowercase = address.to_ascii_lowercase();
    let mut pending_id = String::new();
    for (i, (wire_address, amount)) in [
        (address, "4723.030"),
        (address, "684.6300"),
        (&lowercase, "684.63"),
        (address, "1"),
    ]
    .into_iter()
    .enumerate()
    {
        let req = WithdrawRequest {
            venue: VenueId("mexc".into()),
            asset: Asset {
                ticker: "NXT".into(),
                chain: None,
                contract: None,
            },
            network: "BSC".into(),
            address: wire_address.to_string(),
            amount: amount.parse().unwrap(),
            deal_id: format!("2026112608105034{i}NXT"),
        };
        let out = transfers::withdraw(&ctx, req).await;
        match i {
            0 => assert_eq!(out.unwrap_err().code, ErrorCode::InvalidAddress),
            3 => {
                let error = out.unwrap_err();
                assert_eq!(error.code, ErrorCode::OutcomeUnknown);
                pending_id = error.pending.unwrap().client_id.unwrap();
            }
            _ => assert_eq!(out.unwrap().venue_ref, "fixture-mexc-id"),
        }
    }
    let mut entries = journal.list(100, None).await.unwrap();
    entries.retain(|e| e.kind == "mexc.withdraw_attempt");
    entries.reverse();
    let wire = requests.lock().unwrap();
    assert_eq!(wire.len(), 4);
    assert_eq!(histories.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(wire[3]["withdrawOrderId"], pending_id);
    assert_eq!(entries.len(), wire.len());
    for (i, (entry, params)) in entries.iter().zip(wire.iter()).enumerate() {
        let msg = &entry.message;
        let amount = ["4723.03", "684.63", "684.63", "1"][i];
        assert_eq!(params["amount"], amount);
        assert!(msg.contains(&format!("{amount} NXT")));
        assert!(msg.contains(&format!("netWork={}", params["netWork"])));
        assert!(msg.contains(&format!("2026112608105034{i}NXT:")));
        assert!(msg.contains(&format!("withdrawOrderId={}", params["withdrawOrderId"])));
        assert!(msg.contains(&hex::encode(Sha256::digest(params["address"].as_bytes()))));
        assert!(msg.contains(
            [
                "InvalidAddress",
                "получен ID вывода",
                "получен ID вывода",
                "OutcomeUnknown"
            ][i]
        ));
        assert!(!msg.contains(&params["address"]));
        assert!(!msg.contains(&params["signature"]));
        assert!(!msg.contains("fixture-mexc.api"));
    }
    server.abort();
}
