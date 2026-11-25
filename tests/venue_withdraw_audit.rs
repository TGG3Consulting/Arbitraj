//! Control the withdrawal diagnostics through signed HTTP against recorded exchange error codes.
use arb::core::types::{Asset, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::settings::{store::Store, value::Scope};
use arb::storage::{db::Db, journal::Journal, raw_log::RawLog};
use rust_decimal::Decimal;
use serde_json::json;
use std::path::Path;
use std::str::FromStr;
use tokio::sync::broadcast;

#[tokio::test]
async fn scoped_account_responses_and_failures() {
    gate_rain_repeat_uses_local_account_scoped_denial_and_survives_restart().await;
    binance_cyber_refusals_preserve_retry_and_report_exact_submitted_amount().await;
}

struct Env {
    settings: Store,
    journal: Journal,
    raw: RawLog,
    vault: arb::secrets::Vault,
}

async fn env(name: &str) -> Env {
    let d = std::env::temp_dir().join(format!("arb-238a-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let raw = RawLog::start(d.join("raw"), settings.clone());
    let vault = arb::secrets::Vault::locked(Path::new(&d).join("secrets.enc"));
    Env {
        settings,
        journal,
        raw,
        vault,
    }
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn asset(t: &str) -> Asset {
    Asset {
        chain: None,
        ticker: t.into(),
        contract: None,
    }
}

async fn gate_rain_repeat_uses_local_account_scoped_denial_and_survives_restart() {
    use arb::core::types::WithdrawRequest;
    use arb::gateway::contract::Venue;
    use arb::venues::gate::{Gate, ctx::Ctx};
    use axum::{Json, Router, http::StatusCode, routing::post};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use zeroize::Zeroizing;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let queries = Arc::new(AtomicUsize::new(0));
    let reads = queries.clone();
    let app=Router::new()
        .route("/withdrawals",post(move || {
            let call=count.fetch_add(1,Ordering::SeqCst);
            async move{
                match call{
                    0 | 3=>(StatusCode::BAD_REQUEST,Json(json!({"label":"ADDRESS_NOT_USED","message":"Error: only used addresses or verified addresses are allowed for api withdrawal.RAIN-ARBEVM"}))),
                    1=>(StatusCode::BAD_REQUEST,Json(json!({"label":"TOO_FAST","message":"withdrawals too frequent"}))),
                    _=>(StatusCode::OK,Json(json!({"id":"w-manually-fixed"})))
                }
            }
        }))
        .route("/account/detail",axum::routing::get(||async{Json(json!({"key":{"mode":1}}))}))
        .route("/account/main_keys",axum::routing::get(||async{Json(json!([{"key":"key-a","perms":[{"name":"spot","read_only":false},{"name":"wallet","read_only":true}]}]))}))
        .route("/wallet/saved_address",axum::routing::get(move||{
            let verified=if reads.fetch_add(1,Ordering::SeqCst)==0{"0"}else{"1"};
            async move{Json(json!([{"currency":"RAIN","chain":"ARBEVM","address":"0x00000000000000000000000000000000000000ab","tag":"","verified":verified}]))}
        }))
        .route("/wallet/withdrawals",axum::routing::get(||async{Json(json!([]))}));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (e, kdf) = signed_env("rain-rejected-destination", "gate", &url).await;
    e.settings
        .set(
            "venue.api_ip_bound",
            Scope::Venue("gate".into()),
            Some(json!(false)),
            "test",
        )
        .await
        .unwrap();
    let pw = || Zeroizing::new("test".to_owned());
    let journal = e.journal.clone();
    let ctx = Ctx {
        settings: e.settings,
        vault: e.vault.clone(),
        http: reqwest::Client::new(),
        journal: e.journal,
        raw: e.raw,
        taps: Default::default(),
    };
    let adapter = Gate::new(ctx.clone());
    let req = WithdrawRequest {
        venue: VenueId("gate".into()),
        asset: asset("RAIN"),
        network: "ARBEVM".into(),
        address: "0x00000000000000000000000000000000000000Ab".into(),
        amount: Decimal::ONE,
        deal_id: "rain-first".into(),
    };
    adapter
        .check_withdraw_destination(&req.asset, &req.network, &req.address)
        .await
        .unwrap();
    journal.db().ledger().write(|c|c.execute_batch("CREATE TRIGGER gate_fail_insert BEFORE INSERT ON gate_withdraw_denial BEGIN SELECT RAISE(ABORT,'test unable to save'); END")).await.unwrap();
    assert_eq!(
        adapter.withdraw(req.clone()).await.unwrap_err().code,
        ErrorCode::InvalidAddress
    );
    assert!(
        adapter
            .check_withdraw_destination(&req.asset, &req.network, &req.address)
            .await
            .is_err()
    );
    journal
        .db()
        .ledger()
        .write(|c| c.execute_batch("DROP TRIGGER gate_fail_insert"))
        .await
        .unwrap();
    assert_eq!(
        adapter
            .check_withdraw_destination(&req.asset, &req.network, &req.address)
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidAddress
    );
    drop(adapter);
    let adapter = Gate::new(ctx);
    assert!(adapter.checks_cex_withdraw_destination());
    assert_eq!(
        adapter
            .check_withdraw_destination(&req.asset, &req.network, &req.address.to_ascii_lowercase())
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidAddress
    );
    for (coin, net, address) in [
        ("RAIN", "BSC", req.address.as_str()),
        ("RAIN", "ARBEVM", "different"),
        ("OTHER", "ARBEVM", req.address.as_str()),
    ] {
        adapter
            .check_withdraw_destination(&asset(coin), net, address)
            .await
            .unwrap();
    }
    e.vault
        .change(
            pw(),
            "gate.api_key",
            Some(Zeroizing::new("key-b".into())),
            kdf,
        )
        .await
        .unwrap();
    adapter
        .check_withdraw_destination(&req.asset, &req.network, &req.address)
        .await
        .unwrap();
    let err = adapter.withdraw(req.clone()).await.unwrap_err();
    assert_ne!(err.code, ErrorCode::InvalidAddress);
    adapter
        .check_withdraw_destination(&req.asset, &req.network, &req.address)
        .await
        .unwrap();
    e.vault
        .change(
            pw(),
            "gate.api_key",
            Some(Zeroizing::new("key-a".into())),
            kdf,
        )
        .await
        .unwrap();
    assert!(
        adapter
            .check_withdraw_destination(&req.asset, &req.network, &req.address)
            .await
            .is_err()
    );
    // Existing holdings can still be withdrawn. Explicit acceptance for this key clears its remembered rejection.
    adapter.withdraw(req.clone()).await.unwrap();
    adapter
        .check_withdraw_destination(&req.asset, &req.network, &req.address)
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        adapter.withdraw(req.clone()).await.unwrap_err().code,
        ErrorCode::InvalidAddress
    );
    let first = adapter.check_connection().await.unwrap();
    assert!(first.ok, "{}", first.details);
    assert!(
        adapter
            .check_withdraw_destination(&req.asset, &req.network, &req.address)
            .await
            .is_err()
    );
    let second = adapter.check_connection().await.unwrap();
    assert!(second.ok, "{}", second.details);
    adapter
        .check_withdraw_destination(&req.asset, &req.network, &req.address)
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(queries.load(Ordering::SeqCst), 2);
}

async fn binance_cyber_refusals_preserve_retry_and_report_exact_submitted_amount() {
    use arb::core::types::WithdrawRequest;
    use arb::venues::binance::{ctx::Ctx, transfers, withdraw_ready};
    use axum::{
        Json, Router,
        http::StatusCode,
        routing::{get, post},
    };
    use std::time::Duration;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route(
            "/api/v3/account",
            get(|| async {
                Json(json!({"balances":[{"asset":"CYBER","free":"99","locked":"1.5"}]}))
            }),
        )
        .route(
            "/sapi/v1/capital/withdraw/apply",
            post(|| async {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"code":-4026,"msg":"[031033] User has insufficient balance"})),
                )
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (e, _) = signed_env("cyber-refused-withdrawal", "binance", &url).await;
    for (key, value) in [
        ("venue.withdraw_wallet", json!("spot")),
        ("venue.recv_window_ms", json!(5000)),
        ("venue.withdraw_history_min_ms", json!(100)),
        ("venue.withdraw_unready_window_ms", json!(5000)),
    ] {
        e.settings
            .set(key, Scope::Venue("binance".into()), Some(value), "test")
            .await
            .unwrap();
    }
    let ctx = Ctx {
        settings: e.settings,
        vault: e.vault,
        http: reqwest::Client::new(),
        journal: e.journal,
        raw: e.raw,
        taps: Default::default(),
    };
    let coin = asset("CYBER");
    withdraw_ready::wait(&ctx, &coin, Decimal::ONE, Duration::ZERO)
        .await
        .unwrap();
    // Four observed purchases share this exchange reply. The archive lacks their signed POSTs.
    // Supply the four computed net quantities as inputs and verify exactly what this release sends.
    for amount in ["83.26665", "66.14379", "85.74417", "84.42549"] {
        withdraw_ready::traded("CYBER", tokio::time::Instant::now());
        let req = WithdrawRequest {
            venue: VenueId("binance".into()),
            asset: coin.clone(),
            amount: dec(amount),
            network: "BSC".into(),
            address: "private-withdrawal-address".into(),
            deal_id: "20261110120442028CYBER".into(),
        };
        let err = transfers::withdraw(&ctx, req).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::WithdrawLocked, "{}", err.message);
        for expected in [
            amount,
            "BSC",
            "walletType=0",
            "[031033]",
            "свободно 99",
            "заблокировано 1.5",
            "ответ t=",
        ] {
            assert!(err.message.contains(expected), "{}", err.message);
        }
        assert!(!err.message.contains("private-withdrawal-address"));
        assert!(!err.message.contains("signature="));
    }
}

async fn signed_env(name: &str, venue: &str, url: &str) -> (Env, arb::secrets::crypto::KdfParams) {
    use arb::secrets::crypto::KdfParams;
    use zeroize::Zeroizing;
    let e = env(name).await;
    let kdf = KdfParams {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };
    let pw = || Zeroizing::new("test".to_string());
    e.vault.create(pw(), kdf).await.unwrap();
    for (key, value) in [
        (format!("{venue}.api_key"), "key-a"),
        (format!("{venue}.api_secret"), "secret"),
    ] {
        e.vault
            .change(pw(), &key, Some(Zeroizing::new(value.into())), kdf)
            .await
            .unwrap();
    }
    for (key, val) in [
        ("http.timeout_ms", 5000),
        ("http.total_ms", 5000),
        ("http.body_idle_ms", 5000),
        ("http.max_body_bytes", 100000),
    ] {
        e.settings
            .set(key, Scope::Global, Some(json!(val)), "test")
            .await
            .unwrap();
    }
    e.settings
        .set(
            "venue.rest_url",
            Scope::Venue(venue.into()),
            Some(json!(url)),
            "test",
        )
        .await
        .unwrap();
    (e, kdf)
}
