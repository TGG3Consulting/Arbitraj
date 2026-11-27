//! Локальный RPC воспроизводит два реальных списка PIG. Указатель, смена корня
//! и отказ узла — контролируемые условия теста, а не исторические события PIG.
use super::{
    tests::{fixture, parse},
    *,
};
use crate::{settings::scope::Scope, venues::bsc_dex::hashflow::recovery_tests};
use axum::{Json, Router, routing::post};
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
};

#[tokio::test]
async fn changed_pointer_rediscovery_and_conditional_negative_reuse() {
    let pig = fixture("pig");
    let chain = pig["chain_id"].as_u64().unwrap();
    let token: Address = parse(&pig["token"]);
    let wallet: Address = parse(&pig["samples"][0]["holder"]);
    let dir = recovery_tests::directory("presell-rebind", chain);
    let parts = recovery_tests::parts(&dir, chain, wallet).await;
    let ctx = &parts.ctx;
    let sock = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", sock.local_addr().unwrap());
    let accesses = Arc::new(AtomicUsize::new(0));
    let unavailable = Arc::new(AtomicBool::new(false));
    let (count, fail) = (accesses.clone(), unavailable.clone());
    let first = B256::repeat_byte(0x11);
    let second = B256::repeat_byte(0x22);
    let third = B256::repeat_byte(0x33);
    let handler = move |Json(q): Json<Value>| {
        let (pig, count, fail) = (pig.clone(), count.clone(), fail.clone());
        async move {
            let p = &q["params"];
            let result = match q["method"].as_str().unwrap() {
                "eth_chainId" => json!(format!("{chain:#x}")),
                "eth_getStorageAt" => {
                    assert_eq!(p[1], json!(IMPL_SLOT));
                    if fail.load(SeqCst) {
                        return Json(
                            json!({"jsonrpc":"2.0","id":q["id"],"error":{"code":-32000,"message":"recorded node failure"}}),
                        );
                    }
                    json!(Address::repeat_byte(8).into_word())
                }
                "eth_createAccessList" => {
                    count.fetch_add(1, SeqCst);
                    assert_eq!(p[1], json!({"blockHash":first,"requireCanonical":true}));
                    pig["samples"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|r| r["access"]["request"]["params"][0]["data"] == p[0]["data"])
                        .unwrap()["access"]["response"]["result"]
                        .clone()
                }
                "eth_getProof" => {
                    assert_eq!(p[0], json!(token));
                    assert_eq!(p[1], json!([]));
                    assert_eq!(p[2]["requireCanonical"], true);
                    let root = if p[2]["blockHash"] == json!(third) {
                        third
                    } else {
                        first
                    };
                    json!({"address":token,"codeHash":first,"storageHash":root,"balance":"0x0"})
                }
                method => panic!("unexpected RPC {method}"),
            };
            Json(json!({"jsonrpc":"2.0","id":q["id"],"result":result}))
        }
    };
    let server = tokio::spawn(async move {
        axum::serve(sock, Router::new().route("/", post(handler)))
            .await
            .unwrap()
    });
    let venue = Scope::Venue(ctx.place.id.into());
    for (key, scope, value) in [
        ("dex.chain", venue.clone(), json!("bsc")),
        ("dex.evm_chain_id", venue.clone(), json!(chain)),
        ("http.timeout_ms", Scope::Global, json!(5000)),
        ("http.body_idle_ms", Scope::Global, json!(5000)),
        ("http.total_ms", Scope::Global, json!(8000)),
        ("http.max_body_bytes", Scope::Global, json!(2_000_000)),
        ("bsc.node_http_url", venue.clone(), json!(endpoint)),
        ("bsc.node_http_rps", venue, json!(1000)),
    ] {
        ctx.settings
            .set(key, scope, Some(value), "test")
            .await
            .unwrap();
    }
    let generation = ctx.settings.generation();
    let cache = Cache::default();
    let head = |hash| Head {
        hash,
        number: alloy_primitives::U64::from(1u64),
        timestamp: alloy_primitives::U64::from(1u64),
    };
    let old = Some((
        Identity {
            runtime: first,
            target: Some((Address::repeat_byte(7), first)),
            implementation_slot: true,
            admin: None,
        },
        BalanceField {
            storage: token,
            key: first,
            max: U256::MAX,
        },
    ));
    // Сетевая ошибка при чтении указателя не означает смену и не кэшируется.
    unavailable.store(true, SeqCst);
    let result = verify(ctx, token, wallet, head(first), generation, &cache, old).await;
    assert!(matches!(result, Err(Denial::Temporary(_))));
    assert_eq!(accesses.load(SeqCst), 0);
    unavailable.store(false, SeqCst);
    let Err(reason) = verify(ctx, token, wallet, head(first), generation, &cache, old).await else {
        panic!("reflection")
    };
    let (why, negative) = reason.record(ctx, generation, head(first), &cache).await;
    assert!(why.contains("общие чтения") || why.contains("отдельных ключей"));
    assert_eq!(accesses.load(SeqCst), 2);
    let negative = negative.unwrap();
    assert!(
        negative
            .unchanged(ctx, generation, head(second), &cache)
            .await
            .unwrap()
    );
    assert!(
        !negative
            .unchanged(ctx, generation, head(third), &cache)
            .await
            .unwrap()
    );
    assert_eq!(accesses.load(SeqCst), 2);
    server.abort();
}
