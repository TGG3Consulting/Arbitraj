//! Before-control for Gate ACE on the unmodified 2925a361 source tree.
use arb::core::types::{Asset, Side};
use arb::gateway::errors::ErrorCode;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::broadcast;

struct Env {
    db: Db,
    changes: broadcast::Sender<arb::settings::live::Change>,
    dir: std::path::PathBuf,
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
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let raw = RawLog::start(d.join("raw"), settings.clone());
    let vault = arb::secrets::Vault::locked(Path::new(&d).join("secrets.enc"));
    Env {
        db,
        changes,
        dir: d,
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

fn read(p: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[tokio::test]
async fn before_gate_ace_missing_fee_and_stream_book() {
    use arb::gateway::contract::Venue;
    use axum::{Json, Router, routing::get};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let e = env("gate-quote-stages").await;
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let levels = read("tests/data/cex_samples_2026_11_04/gate_spot_order_book_CAKE_USDT.json");
    let markets = read("tests/data/cex_samples_2026_11_04/gate_spot_currency_pairs.json");
    let ace_market = markets
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "ACE_USDT")
        .unwrap()
        .clone();
    let fee_reply = Arc::new(tokio::sync::Notify::new());
    let release = fee_reply.clone();
    let fee_started = Arc::new(tokio::sync::Notify::new());
    let started = fee_started.clone();

    let app = Router::new()
        .route(
            "/spot/currency_pairs",
            get(move || async move { Json(json!([ace_market])) }),
        )
        .route(
            "/account/detail",
            get(|| async { Json(json!({"currency_pairs":[]})) }),
        )
        .route(
            "/wallet/fee",
            get(move || async move {
                started.notify_one();
                release.notified().await;
                Json(json!({"maker_fee":"0.001","taker_fee":"0.001"}))
            }),
        )
        .route(
            "/spot/order_book",
            get(move || {
                let body = if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    json!({})
                } else {
                    levels.clone()
                };
                async move { Json(body) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for (key, val) in [
        ("http.timeout_ms", 5000),
        ("http.total_ms", 5000),
        ("http.body_idle_ms", 5000),
        ("http.max_body_bytes", 100000),
    ] {
        e.settings
            .set(key, Scope::Global, Some(json!(val)), "тест")
            .await
            .unwrap();
    }
    for (key, val) in [
        ("venue.rest_url", json!(url)),
        ("venue.quote_asset", json!("USDT")),
        ("venue.depth_levels", json!(20)),
        ("venue.ws_symbols_per_conn", json!(5)),
        ("venue.book_interval", json!("100ms")),
        ("venue.symbols_refresh_s", json!(3600)),
        ("venue.fee_requests_per_sec", json!(5)),
        ("venue.fee_cache_s", json!(3600)),
        ("venue.fee_retry_s", json!(1)),
        ("venue.fee_deduction", json!("coin")),
        ("venue.book_stale_ms", json!(5000)),
        ("feed.stale_after_ms", json!(30000)),
        ("exec.rest_book_retry_ms", json!(10000)),
    ] {
        e.settings
            .set(key, Scope::Venue("gate".into()), Some(val), "тест")
            .await
            .unwrap();
    }
    let ctx = arb::venues::gate::ctx::Ctx {
        settings: e.settings.clone(),
        vault: e.vault.clone(),
        journal: e.journal.clone(),
        raw: e.raw,
        http: reqwest::Client::new(),
        taps: Default::default(),
    };
    let gate = Arc::new(arb::venues::gate::Gate::new(ctx.clone()));
    let coin = asset("ACE");

    let bad_book = gate.quote_rest(&coin, Side::Sell).await.err().unwrap();
    assert_eq!(bad_book.code, ErrorCode::BadResponse);
    let fees = arb::venues::gate::fees::Fees::default();
    let books = arb::venues::gate::book::Books::default();
    let absent = || {
        books
            .freeze(&ctx, &fees, &coin, Side::Sell)
            .unwrap_err()
            .message
    };
    let unsubscribed = absent();
    let (ws, _rx) = books.feed.register("market-2", &["ACE".to_owned()]);
    ws.session_started();
    assert_eq!(absent(), unsubscribed);
    ws.frame("ACE");
    assert_eq!(absent(), unsubscribed);
    assert_eq!(unsubscribed, "Gate ACE: стакана нет — котировки нет");
    let missing_fee = gate.quote_rest(&coin, Side::Sell).await.err().unwrap();
    assert_eq!(missing_fee.code, ErrorCode::StaleData);
    assert!(
        missing_fee
            .message
            .contains("ACE_USDT: ставка комиссии ещё не получена")
    );
    assert!(!missing_fee.message.contains("получен и разобран"));
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    let gw = arb::gateway::registry::Registry::new();
    gw.register(gate.clone()).await;
    let notifier = arb::notify::dispatch::Notifier {
        http: reqwest::Client::new(),
        settings: e.settings.clone(),
        vault: e.vault.clone(),
        journal: e.journal.clone(),
    };
    let core = arb::core::Core::start(arb::core::deps::Deps {
        gw,
        settings: e.settings.clone(),
        journal: e.journal,
        notifier,
        db: e.db,
        changes: e.changes,
        buffer: 64,
        data_dir: e.dir,
    })
    .await;
    let venue = gate.id();
    let quote = || arb::core::exec_book::book(&core, &venue, &coin, Side::Sell);

    let started = std::time::Instant::now();
    let message = quote().await.err().unwrap().message;
    assert!(
        message.contains("ACE_USDT: ставка комиссии ещё не получена"),
        "{message}"
    );
    assert!(message.contains("повтор через 10000 мс"), "{message}");
    assert_eq!(requests.load(Ordering::SeqCst), 4);
    let initial_duration_ms = started.elapsed().as_millis();
    let cooldown_started = std::time::Instant::now();
    let cooldown = quote().await.err().unwrap().message;
    assert!(cooldown.contains("не запрашивался"), "{cooldown}");
    assert_eq!(requests.load(Ordering::SeqCst), 4);
    let kdf = arb::secrets::crypto::KdfParams {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };
    let pw = || zeroize::Zeroizing::new("gate fixture".to_owned());
    e.vault.create(pw(), kdf).await.unwrap();
    for name in ["gate.api_key", "gate.api_secret"] {
        e.vault
            .change(
                pw(),
                name,
                Some(zeroize::Zeroizing::new("k".to_owned())),
                kdf,
            )
            .await
            .unwrap();
    }
    let (out, _keep_rx) = tokio::sync::mpsc::channel(32);
    gate.subscribe_market(vec![coin.clone()], out)
        .await
        .unwrap();
    let wait = std::time::Duration::from_secs(5);
    tokio::time::timeout(wait, fee_started.notified())
        .await
        .unwrap();

    fee_reply.notify_one();
    // The baseline has no public method to inspect the fee cache. Only the independent,
    // directly executable REST quote below can prove it has already been populated.
    let direct = tokio::time::timeout(wait, async {
        loop {
            if let Ok(result) = gate.quote_rest(&coin, Side::Sell).await {
                break result;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let priced = direct.quote(dec("1")).await.unwrap();
    assert_eq!(priced.costs[0].amount, dec("0.0018902"));
    let after_proof = requests.load(Ordering::SeqCst);
    let still_blocked = quote().await.err().unwrap().message;
    assert!(
        still_blocked.contains("не запрашивался") && still_blocked.contains("ставка комиссии"),
        "{still_blocked}"
    );
    let after_cooldown = requests.load(Ordering::SeqCst);
    assert_eq!(after_cooldown, after_proof);
    let waiting_with_ready_fee_ms = cooldown_started.elapsed().as_millis();
    tokio::time::sleep(
        std::time::Duration::from_millis(10_010).saturating_sub(cooldown_started.elapsed()),
    )
    .await;
    let allowed = quote().await.expect("after the historical cooldown");
    assert!(allowed.rest);
    assert_eq!(requests.load(Ordering::SeqCst), after_cooldown + 1);
    let report = json!({
         "production_source": "2925a361bb88edff0ee1aab3e18bdd3e6ff5b843",
         "fixture": "ACE_USDT pair metadata, numeric Gate CAKE_USDT 20-level snapshot, fixture fee 0.001",
         "missing_ws_reasons_equal":true, "original_ws_reason":unsubscribed,
         "core_http_requests_while_fee_missing":2, "retry_ms":10000, "first_core_call_measured_ms":initial_duration_ms,
         "initial_cooldown_gets":0, "direct_attempts_to_prove_fee_warm":after_proof-4,
         "fee_ready_while_cooling_after_ms":waiting_with_ready_fee_ms, "executable_direct_quote_fee_usdt":priced.costs[0].amount,
         "core_http_requests_with_ready_fee_inside_cooldown":after_cooldown-after_proof,
         "core_quote_with_ready_fee_inside_cooldown":"refused", "core_http_after_cooldown":1,
         "initial_error":message,"cooldown_error":still_blocked,
    });
    if let Some(path) = std::env::var_os("GATE_BEFORE_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    server.abort();
}
