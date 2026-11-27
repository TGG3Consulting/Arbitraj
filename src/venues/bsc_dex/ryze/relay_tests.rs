//! Локальный HTTP relay и реальный SQLite повторяют потерю ответа, конкурирующие ключи и перезапуск.
use super::{
    http,
    ledger::Marker,
    wire::{Hop, SwapIntent},
};
use crate::{
    core::types::{Asset, ChainId, Side, TradeRequest, VenueId},
    gateway::{http::Limits, settlement_intent as intent},
    storage::db::Db,
};
use alloy::{
    primitives::{Address, B256, U256},
    signers::{SignerSync, local::PrivateKeySigner},
};
use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

pub(super) fn limits() -> Limits {
    Limits {
        head: Duration::from_secs(3),
        idle: Duration::from_secs(3),
        total: Duration::from_secs(5),
        max_bytes: 32_768,
    }
}

#[tokio::test]
async fn saved_digest_survives_lost_ack_reopen_and_competing_deals_without_resubmission() {
    let wallet = PrivateKeySigner::random();
    let owner = wallet.address();
    let router = Address::repeat_byte(0x54);
    let coin = Address::repeat_byte(0x12);
    let cash = Address::repeat_byte(0x34);
    let order = SwapIntent {
        user: owner,
        tokenIn: coin,
        tokenOut: cash,
        amountIn: U256::from(127),
        minAmountOut: U256::from(256),
        path: vec![Hop {
            pool: Address::repeat_byte(0x13),
            tokenIn: coin,
            tokenOut: cash,
        }],
        recipient: owner,
        deadline: U256::from(1000),
        nonce: U256::ONE,
    };
    let digest = order.digest(8453, router);
    let sig = wallet.sign_hash_sync(&digest).unwrap();
    let body = order.submission(router, &sig).unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let writes = count.clone();
    let recorded = body.clone();
    let tx = B256::repeat_byte(0xab);
    let server=Router::new().route("/intents/submit",post(move|Json(got):Json<Value>|async move{
        assert_eq!(got,recorded);writes.fetch_add(1,Ordering::SeqCst);StatusCode::SERVICE_UNAVAILABLE}))
        .route("/intents/{id}",get(move||async move{Json(json!({"success":true,"intent":{"intentId":digest,"status":"confirmed","txHash":tx}}))}));
    let socket = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = socket.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(socket, server).await.unwrap() });
    let base = format!("http://{addr}");
    let m = Marker {
        key: "ryze/test-order".into(),
        account: "test".into(),
        router,
        runtime: B256::repeat_byte(42),
        relayer: base.clone(),
        tokens: vec![],
        order,
        digest,
        start: 10,
        start_hash: B256::repeat_byte(10),
        decimals: (8, 6),
    };
    let r = TradeRequest {
        venue: VenueId("base_dex".into()),
        asset: Asset {
            chain: Some(ChainId("base".into())),
            contract: Some(coin),
            ticker: "TEST".into(),
        },
        side: Side::Sell,
        qty: Decimal::ONE,
        worst_price: Decimal::ONE,
        deal_id: "ephemeral".into(),
        route: None,
        exit_position: false,
        remeasure: None,
    };
    let folder = std::env::temp_dir().join(format!("arb-ryze-{}", owner));
    std::fs::create_dir(&folder).unwrap();
    let path = folder.join("test.sqlite");
    let mut b = m.clone();
    b.key.push('B');
    {
        let db = Db::open(&path).unwrap();
        intent::scope(
            &db,
            &m.key,
            intent::Request::Order(Box::new(r.clone())),
            intent::before_send(&m.pending()),
        )
        .await
        .unwrap();
        assert!(m.reserve(&db).await.unwrap());
        assert!(!b.reserve(&db).await.unwrap());
        assert_eq!(
            http::submit_once(
                format!("{base}/intents/submit").parse().unwrap(),
                digest,
                body,
                limits()
            )
            .await,
            http::Submit::Unknown
        );
    }
    {
        let db = Db::open(&path).unwrap();
        let e = intent::load(&db, &m.key).await.unwrap().unwrap();
        assert_eq!(Marker::read(&e.lookup).unwrap().digest, digest);
        assert!(
            !serde_json::to_string(&e)
                .unwrap()
                .contains(&sig.to_string())
        );
        assert!(
            intent::scope(
                &db,
                &m.key,
                intent::Request::Order(Box::new(r)),
                intent::before_send(&b.pending())
            )
            .await
            .is_err()
        );
        b.release(&db).await.unwrap();
        assert_eq!(m.holder(&db).await.unwrap(), Some(m.claim()));
        assert!(m.was_reserved(&db).await.unwrap());
        assert!(!b.was_reserved(&db).await.unwrap());
        let url = format!("{base}/intents/{digest}").parse().unwrap();
        assert!(
            matches!(http::transaction(url, digest, limits()).await, http::Status::Hint(Some(id)) if id==tx)
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        m.release(&db).await.unwrap();
        assert!(b.reserve(&db).await.unwrap());
    }
    task.abort();
    std::fs::remove_dir_all(folder).unwrap();
}
