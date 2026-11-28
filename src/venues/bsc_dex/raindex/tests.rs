//! Сверка исходного хеша ордера, двух событий и физических остатков по обеим сетям.
use super::*;
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent, SolValue},
};
use flate2::read::GzDecoder;
use serde_json::{Value, from_value};

#[path = "deployment_tests.rs"]
mod deployments;
#[path = "capacity_tests.rs"]
mod capacity;
#[path = "reverse_tests.rs"]
mod reverse;
#[path = "history_tests.rs"]
mod history;

fn amount(row: &Value, field: &str) -> U256 {
    row[field].as_str().unwrap().parse().unwrap()
}

fn check(row: &Value, owner: Address, chain: u64) -> payment::Saved {
    let module = from_value(row["target"].clone()).unwrap();
    let book = from_value(row["book"].clone()).unwrap();
    let p = Profile {
        module,
        book,
        chain,
        version: row["version"].as_u64().unwrap() as u8,
        book_hash: B256::ZERO,
        module_hash: B256::ZERO,
    };
    let action: Bytes = from_value(row["action"].clone()).unwrap();
    let (encoded, topic) = if p.version == 3 {
        let c = wire::takeOrdersCall::abi_decode_validate(&action)
            .unwrap()
            .config;
        assert_eq!(c.orders.len(), 1);
        (
            c.orders[0].order.abi_encode(),
            wire::TakeOrder::SIGNATURE_HASH,
        )
    } else if p.version == 4 {
        let c = wire::takeOrders2Call::abi_decode_validate(&action)
            .unwrap()
            .config;
        assert_eq!(c.orders.len(), 1);
        (
            c.orders[0].order.abi_encode(),
            wire::TakeOrderV2::SIGNATURE_HASH,
        )
    } else {
        let c = wire::takeOrders4Call::abi_decode_validate(&action)
            .unwrap()
            .config;
        assert_eq!(c.orders.len(), 1);
        (
            c.orders[0].order.abi_encode(),
            wire::TakeOrderV3::SIGNATURE_HASH,
        )
    };
    let hash = from_value(row["order"].clone()).unwrap();
    let order = Arc::new(wire::Order::decode(p.version, &encoded, hash).unwrap());
    let leg = Leg {
        via: None,
        p,
        hash,
        order,
        i: row["i"].as_u64().unwrap() as usize,
        j: row["j"].as_u64().unwrap() as usize,
        input: from_value(row["pay"].clone()).unwrap(),
        output: from_value(row["receive"].clone()).unwrap(),
    };
    let spent = amount(row, "paid");
    let net = amount(row, "net");
    let payment = payment::Payment::new(&leg, owner, action, 0, (spent, spent, net));
    let mut logs: Vec<Log> = from_value(row["logs"].clone()).unwrap();
    assert_eq!(payment.saved.amounts(&logs), Some((spent, net)));
    let holdings = |key: &str| {
        row[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().parse::<U256>().unwrap())
            .collect::<Vec<_>>()
    };
    let before = holdings("before");
    let after = holdings("after");
    assert_eq!(before[0] - after[0], spent);
    assert_eq!(after[1] - before[1], net);
    assert_eq!(before[2..], after[2..]);
    let repeated = logs
        .iter()
        .find(|e| e.address() == book && e.topics().first() == Some(&topic))
        .unwrap()
        .clone();
    logs.push(repeated);
    assert!(payment.saved.amounts(&logs).is_none());
    payment.saved
}

#[test]
fn original_order_and_one_payment_match_own_wallet_on_both_networks() {
    let base = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_protected_wallets.json.gz"
    );
    let eth = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_ethereum_protected_wallets.json.gz"
    );
    let old = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_v3_ethereum.json.gz"
    );
    for (archive, chain, expected) in [
        (base.as_slice(), 8453, (42, 7)),
        (eth.as_slice(), 1, (3, 0)),
        (old.as_slice(), 1, (4, 0)),
    ] {
        let j: Value = serde_json::from_reader(GzDecoder::new(archive)).unwrap();
        let owner = from_value(j["payer"].clone()).unwrap();
        let mut counts = (0, 0);
        for pair in j["pairs"].as_array().unwrap() {
            for case in pair["cases"].as_array().unwrap() {
                for row in case
                    .as_object()
                    .unwrap()
                    .values()
                    .filter(|r| r["stage"] == "paid")
                {
                    check(row, owner, chain);
                    counts.0 += 1;
                }
            }
            let partial = &pair["partial_capacity"];
            if partial["stage"] == "paid" {
                assert!(amount(partial, "net") < amount(partial, "size"));
                assert!(amount(partial, "paid") < amount(partial, "cap"));
                check(partial, owner, chain);
                counts.1 += 1;
            }
        }
        assert_eq!(counts, expected);
    }
}

#[test]
fn stablecoin_route_restores_receipt_for_book_fill_and_conversion_of_its_change() {
    let a = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_cash_ethereum.json.gz"
    );
    let data: Value = serde_json::from_reader(GzDecoder::new(a.as_slice())).unwrap();
    let role = cash::Role {
        wrapper: from_value(data["wrapper"].clone()).unwrap(),
        hash: from_value(data["wrapperCode"].clone()).unwrap(),
        router: from_value(data["router"].clone()).unwrap(),
        pair: from_value(data["pair"].clone()).unwrap(),
        cash: from_value(data["cash"].clone()).unwrap(),
        bridge: from_value(data["bridge"].clone()).unwrap(),
    };
    let bare = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_ethereum_protected_wallets.json.gz"
    );
    let book_info: Value = serde_json::from_reader(GzDecoder::new(bare.as_slice())).unwrap();
    let book = book_info["modules"]
        .as_object()
        .unwrap()
        .values()
        .find(|x| x["version"] == 4)
        .unwrap();
    let p = Profile {
        book: from_value(book["book"].clone()).unwrap(),
        book_hash: B256::ZERO,
        chain: 1,
        version: 4,
        module: from_value(data["wallet"].clone()).unwrap(),
        module_hash: from_value(data["walletCode"].clone()).unwrap(),
    };
    let owner = from_value(data["payer"].clone()).unwrap();
    let mut with_return = 0;
    let rows = data["cases"].as_array().unwrap();
    for row in rows {
        let action: Bytes = from_value(row["action"].clone()).unwrap();
        let config = wire::takeOrders2Call::abi_decode_validate(&action)
            .unwrap()
            .config;
        let hash = from_value(row["order"].clone()).unwrap();
        let raw = config.orders[0].order.abi_encode();
        let leg = Leg {
            via: Some(cash::State::archived(role, 18)),
            p,
            hash,
            order: Arc::new(wire::Order::decode(4, &raw, hash).unwrap()),
            i: config.orders[0].inputIOIndex.to(),
            j: config.orders[0].outputIOIndex.to(),
            input: role.bridge,
            output: from_value(row["asset"].clone()).unwrap(),
        };
        let cap = amount(row, "cap");
        let middle = amount(row, "acquired");
        let net = amount(row, "received");
        let spent = amount(row, "spent");
        let payment =
            payment::Payment::route(&leg, owner, action, 0, (U256::ONE, cap, net), middle);
        let logs: Vec<Log> = from_value(row["logs"].clone()).unwrap();
        assert_eq!(payment.saved.amounts(&logs), Some((spent, net)));
        let separate: Vec<_> = logs
            .into_iter()
            .filter(|l| l.address() != p.module)
            .collect();
        assert!(payment.saved.amounts(&separate).is_none());
        if middle > amount(row, "bookPaid") {
            with_return += 1;
        }
    }
    assert_eq!((rows.len(), with_return), (6, 3));
}

#[test]
fn public_v7_claims_decode_with_their_own_abi_and_do_not_create_spot_prices() {
    let src = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_ethereum_quotes.json.gz"
    );
    let terms = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_v7_ethereum_terms.json.gz"
    );
    let j: Value = serde_json::from_reader(GzDecoder::new(src.as_slice())).unwrap();
    let t: Value = serde_json::from_reader(GzDecoder::new(terms.as_slice())).unwrap();
    let book = j["books"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["version"] == 7)
        .unwrap();
    let orders = book["live"].as_array().unwrap();
    assert_eq!(orders.len(), 5);
    for row in orders {
        let terms = t["orders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["order"] == row["hash"])
            .unwrap();
        let raw: Bytes = from_value(terms["metadata"].clone()).unwrap();
        assert_eq!(
            alloy_primitives::keccak256(&raw),
            from_value::<B256>(terms["content_hash"].clone()).unwrap()
        );
        let source = String::from_utf8_lossy(&raw);
        assert!(
            source.contains("order-counterparty()")
                && source.contains("signed-context<0 1>()")
                && source.contains("_: 0;")
        );
        let raw: Bytes = from_value(row["order"].clone()).unwrap();
        let hash = from_value(row["hash"].clone()).unwrap();
        let order = wire::Order::decode(7, &raw, hash).unwrap();
        let q = order.quote(0, 0).unwrap();
        assert_eq!(&q[..4], &wire::quote3Call::SELECTOR);
        let action = order.take(0, 0, U256::ONE, 18, true, B256::ZERO).unwrap();
        assert_eq!(&action[..4], &[0xb0, 0x46, 0x0f, 0x07]);
        let c = wire::takeOrders5Call::abi_decode_validate(&action)
            .unwrap()
            .config;
        assert!(c.data.is_empty() && c.orders[0].signedContext.is_empty());
        let response = (true, Quantity::units(U256::ONE, 0).unwrap(), B256::ZERO).abi_encode();
        assert!(order.price(&response).is_none());
    }
}

#[test]
fn only_an_executable_mixed_book_ticket_selects_the_outer_settlement_action() {
    use crate::core::types::{DexRoute, RouteMode};
    let id = B256::repeat_byte(23);
    let mut route = DexRoute {
        mode: routing::mode(true, Some(id)),
        legs: vec![],
        block: 101,
        gas_units: 100,
        executable: true,
        exec_note: Some(LABEL.into()),
    };
    route.exec_note = Some("Другой текст карточки".into());
    assert_eq!(routing::mixed_ticket(&route), Some(id));
    route.executable = false;
    assert_eq!(routing::mixed_ticket(&route), None);
    route.executable = true;
    for mode in [
        routing::mode(false, Some(id)),
        routing::mode(false, None),
        routing::mode(true, None),
    ] {
        route.mode = mode;
        assert_eq!(routing::mixed_ticket(&route), None);
    }
    route.mode = RouteMode::Orderbook(id);
    route.exec_note = Some("Hanji".into());
    assert_eq!(routing::mixed_ticket(&route), None);
}

#[tokio::test]
async fn lost_response_keeps_the_original_outer_action_after_reopening_the_database() {
    use crate::{
        core::types::{Asset, ChainId, DexRoute, Side, TradeRequest},
        gateway::{pending::PendingRef, settlement_intent as intent},
        venues::bsc_dex::{
            exec::swap_ctx::SwapCtx,
            hashflow::{
                native_send,
                recovery_tests::{directory, parts},
            },
        },
    };
    use rust_decimal::Decimal;
    let bytes = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_protected_wallets.json.gz"
    );
    let j: Value = serde_json::from_reader(GzDecoder::new(bytes.as_slice())).unwrap();
    let row = j["pairs"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["cases"].as_array().unwrap())
        .flat_map(|s| s.as_object().unwrap().values())
        .find(|x| x["stage"] == "paid")
        .unwrap();
    let owner = from_value(j["payer"].clone()).unwrap();
    let saved = check(row, owner, 8453);
    let (input, output) = (
        from_value(row["pay"].clone()).unwrap(),
        from_value(row["receive"].clone()).unwrap(),
    );
    let dir = directory("raindex-mixed", 8453);
    let p = parts(&dir, 8453, owner).await;
    let mut parse = SwapCtx::swap(owner, (input, output), (18, 6), &[], None, None);
    parse.raindex = Some(Box::new(saved));
    let id = B256::repeat_byte(47);
    let req = TradeRequest {
        venue: p.ctx.venue(),
        asset: Asset {
            chain: Some(ChainId("base".into())),
            ticker: "owned".into(),
            contract: Some(output),
        },
        side: Side::Buy,
        qty: Decimal::ONE,
        worst_price: Decimal::ONE,
        deal_id: "interrupted".into(),
        exit_position: false,
        remeasure: None,
        route: Some(DexRoute {
            mode: routing::mode(true, Some(id)),
            legs: vec![],
            block: 62262756,
            gas_units: 100,
            executable: true,
            exec_note: Some(LABEL.into()),
        }),
    };
    let marker = PendingRef {
        nonce: Some(3),
        venue_ref: Some(B256::repeat_byte(92).to_string()),
        parse: Some(serde_json::to_value(&parse).unwrap()),
        ..Default::default()
    };
    let outer = "mixed/archive/own-original";
    let db = p.ctx.journal.db();
    let attempted = std::cell::Cell::new(0_u8);
    let send = || async {
        intent::before_send(&marker).await?;
        attempted.set(attempted.get() + 1);
        Err(p.ctx.err(ErrorCode::OutcomeUnknown, "потерян ответ"))
    };
    for _ in 0..2 {
        let result = intent::scope(
            db,
            outer,
            intent::Request::Order(Box::new(req.clone())),
            native_send(&p, &req, u64::MAX, 0, &parse, send()),
        )
        .await;
        assert_eq!(
            result.err().unwrap().pending.unwrap().venue_ref,
            marker.venue_ref
        );
    }
    assert_eq!(attempted.get(), 1);
    // Сохраняем ту же исходную ссылку и оплаченный контекст в схеме старого журнала.
    db.ledger().write(move |c| c.execute(
        "UPDATE core_state SET body=json_set(body,'$.operation.Order.route.mode',json(?1)) WHERE id=?2",
        rusqlite::params![serde_json::json!({"Orderbook":id}).to_string(), outer],
    )).await.unwrap();
    drop(p);
    let restored = parts(&dir, 8453, owner).await;
    let db = restored.ctx.journal.db();
    let old = intent::load(db, outer).await.unwrap().unwrap();
    let intent::Request::Order(original) = &old.operation else { panic!("ожидается ордер") };
    assert_eq!(original.route.as_ref().and_then(routing::mixed_ticket), Some(id));
    assert_eq!(
        (old.lookup.venue_ref, old.lookup.nonce, old.lookup.parse),
        (marker.venue_ref, marker.nonce, marker.parse)
    );
    let ordinary = format!("hashflow/base_dex/{owner}/{output}/Buy/interrupted");
    assert!(intent::load(db, &ordinary).await.unwrap().is_none());
}
