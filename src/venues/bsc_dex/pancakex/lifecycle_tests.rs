//! Маршрут проходит через реальные несекретные байты ответа, домен сети, устойчивое владение nonce и финализированную границу.
use super::{
    Request,
    assets::AssetCode,
    client::{self, Offer},
    history::{Events, expired},
    ledger::Marker,
    market::{Config, Ticket},
    profile::Profile,
    recovery::closed,
};
use crate::{
    core::types::{Asset, Side, TradeRequest, VenueId},
    gateway::settlement_intent as intent,
    storage::db::Db,
};
use alloy_primitives::{Address, B256, U256};
use flate2::read::GzDecoder;
use rust_decimal::Decimal;
use serde_json::Value;

fn marker() -> Marker {
    let profiles: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/pancakex_profiles.json"
    ))
    .unwrap();
    let book: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/pancakex_unsigned_quotes.json.gz"
        )[..],
    ))
    .unwrap();
    let row = book
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["status"] == 200 && r["body"]["tokenInChainId"] == 8453)
        .unwrap();
    let body = &row["body"];
    let msg = &row["response"];
    let request = Request {
        owner: body["configs"][0]["swapper"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
        input: body["tokenIn"].as_str().unwrap().parse().unwrap(),
        output: body["tokenOut"].as_str().unwrap().parse().unwrap(),
        amount: body["amount"].as_str().unwrap().parse().unwrap(),
        minimum: U256::ONE,
        exact_output: false,
        now: 0,
    };
    let profile = &profiles["base"];
    let rows: Vec<String> = serde_json::from_value(profile["contracts"].clone()).unwrap();
    let p = Profile::from_rows(8453, &rows).unwrap();
    let assets: Vec<_> = profile["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| AssetCode::parse(v.as_str().unwrap(), 8453).unwrap())
        .collect();
    let token = |addr: Address| assets.iter().find(|a| a.address() == addr).unwrap().clone();
    let offer = Offer::read(msg).unwrap();
    let terms = offer.terms(&p, &request).unwrap();
    let cfg = Config {
        p,
        input: token(request.input),
        output: token(request.output),
        base: "https://x.pancakeswap.com".into(),
        bps: 10,
        age: 90,
    };
    let ticket = Ticket {
        cfg,
        req: request,
        offer,
        decimals: (8, 6),
        side: Side::Sell,
        qty: Decimal::ONE,
        block: 100,
        deadline: terms.deadline,
    };
    Marker {
        key: "pcsx/test".into(),
        account: "test".into(),
        ticket,
        start: 99,
        anchor: B256::repeat_byte(7),
        digest: terms.digest,
    }
}
#[test]
fn strict_finalized_deadline_and_unrelated_used_bit_preserve_pending() {
    let m = marker();
    let t = m.terms().unwrap();
    let empty = Events::default();
    assert!(!closed(expired(t.deadline, t.deadline), true, &empty));
    assert!(closed(expired(t.deadline + 1, t.deadline), true, &empty));
    assert!(!closed(true, false, &empty));
    assert!(closed(
        true,
        false,
        &Events {
            excludes: true,
            original: None
        }
    ));
    assert!(!closed(
        true,
        true,
        &Events {
            excludes: true,
            original: Some(m.digest)
        }
    ));
    for bad in [
        "http://x.pancakeswap.com",
        "https://x.pancakeswap.com.other.test",
        "https://x.pancakeswap.com@other.test",
    ] {
        assert!(client::url(bad, "order-handler/order").is_none());
    }
}
#[tokio::test]
async fn lost_ack_and_restart_keep_the_original_intent_and_nonce() {
    let m = marker();
    let t = m.terms().unwrap();
    let folder = std::env::temp_dir().join(format!(
        "arb-pcsx-{}",
        alloy::signers::local::PrivateKeySigner::random().address()
    ));
    std::fs::create_dir(&folder).unwrap();
    let path = folder.join("ledger.sqlite");
    let request = TradeRequest {
        venue: VenueId("base".into()),
        asset: Asset {
            chain: None,
            contract: Some(m.ticket.req.input),
            ticker: "GOOGLc".into(),
        },
        side: Side::Sell,
        qty: Decimal::ONE,
        worst_price: Decimal::ONE,
        deal_id: "local".into(),
        route: None,
        exit_position: false,
        remeasure: None,
    };
    {
        let db = Db::open(&path).unwrap();
        intent::scope(
            &db,
            &m.key,
            intent::Request::Order(Box::new(request.clone())),
            intent::before_send(&m.pending()),
        )
        .await
        .unwrap();
        assert!(m.reserve(&db, &t).await.unwrap());
        // Здесь удалённая сторона могла принять заказ до потери ответа.
    }
    {
        let db = Db::open(&path).unwrap();
        let row = intent::load(&db, &m.key).await.unwrap().unwrap();
        assert!(row.result.is_none());
        assert_eq!(Marker::read(&row.lookup).unwrap().digest, m.digest);
        assert!(
            intent::scope(
                &db,
                &m.key,
                intent::Request::Order(Box::new(request)),
                intent::before_send(&m.pending())
            )
            .await
            .is_err()
        );
        let mut other = m.clone();
        other.key = "another-step".into();
        assert!(!other.reserve(&db, &t).await.unwrap());
        assert_eq!(other.holder(&db, &t).await.unwrap(), Some(m.claim()));
        assert!(!m.reserve(&db, &t).await.unwrap());
    }
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn same_height_reorg_reopens_the_inclusive_deadline() {
    use super::history::{Child, boundary};
    use crate::venues::bsc_dex::quote::white_state::Head;
    let h = Head {
        number: 100u64.into(),
        timestamp: 120u64.into(),
        hash: B256::repeat_byte(1),
    };
    let mut child = Child {
        number: 101u64.into(),
        timestamp: 121u64.into(),
        parent_hash: h.hash,
    };
    assert!(boundary(h, &child, 120));
    child.parent_hash = B256::repeat_byte(2);
    assert!(!boundary(h, &child, 120));
    child.parent_hash = h.hash;
    child.timestamp = 120u64.into();
    assert!(!boundary(h, &child, 120));
}
