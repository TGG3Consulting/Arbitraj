use super::*;
use crate::core::types::{Asset, Cost, VenueId};
use rust_decimal::Decimal;
use serde_json::json;

fn book() -> HistoryCoins {
    let data: Value = serde_json::from_str(include_str!(
        "../../../tests/data/mexc_2026-11-21/config_getall.json"
    ))
    .unwrap();
    let parsed = super::super::networks_parse::parse_networks(
        &data,
        "BSC",
        &VenueId("mexc".into()),
        true,
        &Default::default(),
    )
    .unwrap();
    parsed.history
}

const NETWORK: &str = "BNB Smart Chain(BEP20)";
const CONTRACT: &str = "0x55d398326f99059ff775485246999027b3197955";

#[test]
fn owl_hold_and_final_credit_on_the_recorded_network() {
    let b = book();
    let rec = json!([{"status":9,"coin":"USDT-BSC","network":NETWORK,"amount":"47.080337518416492026","confirmTimes":42,"unlockConfirm":61}]);
    let parse = |r: &Value| super::super::deposit_hold::parse_with(r, Some(&b));
    // Та же запись на исходном парсере уходила в поиск цены торговой пары USDT-BSC.
    assert_eq!(
        super::super::deposit_hold::parse(&rec).unwrap().items[0].asset,
        "USDT-BSC"
    );
    let hold = parse(&rec).unwrap();
    assert_eq!(hold.items.len(), 1);
    let locked = &hold.items[0];
    assert_eq!(locked.asset, "USDT");
    assert_eq!(
        locked.amount,
        "47.080337518416492026".parse::<Decimal>().unwrap()
    );
    for text in ["USDT-BSC", NETWORK, "42", "61", "в расчёте USDT"] {
        assert!(locked.reason.contains(text));
    }
    let verdict = |spend| {
        crate::core::precheck_withdraw::verdict(
            "mexc",
            "USDT",
            Decimal::ONE_HUNDRED,
            locked.amount,
            spend,
            &locked.reason,
        )
    };
    assert!(verdict(Decimal::ONE_HUNDRED - locked.amount).is_ok());
    assert!(
        verdict(Decimal::ONE_HUNDRED - locked.amount + Decimal::new(1, 8))
            .unwrap_err()
            .contains("можно вывести")
    );
    assert_eq!(parse(&rec).unwrap(), hold);
    let mut final_credit = rec.clone();
    final_credit[0]["status"] = json!(5);
    assert!(parse(&final_credit).unwrap().items.is_empty());
    let mut canonical = rec.clone();
    canonical[0]["coin"] = json!("USDT");
    assert_eq!(parse(&canonical).unwrap().items[0].asset, "USDT");
    for source in [None, Some(&b)] {
        let ordinary = json!([{"status":9,"coin":"PLAIN","amount":"1"}]);
        let hold = super::super::deposit_hold::parse_with(&ordinary, source).unwrap();
        assert_eq!(hold.items[0].asset, "PLAIN");
        assert_eq!(hold.items[0].amount, Decimal::ONE);
    }
    let mut unknown = rec;
    unknown[0]["network"] = json!("другая сеть");
    assert!(
        parse(&unknown)
            .unwrap_err()
            .message
            .contains("доступно к выводу неизвестно")
    );
}

#[test]
fn collisions_are_scoped_to_the_network_and_never_guessed_from_suffixes() {
    let mut b = book();
    assert_eq!(b.ticker("BSC", "FOO-BSC"), None);
    b.insert(
        "OTHER",
        "OTHER_NET",
        "Another",
        &json!({"coin":"OTHER","name":"USDT-BSC"}),
    );
    assert_eq!(b.ticker("OTHER_NET", "USDT-BSC"), Some("OTHER"));
    assert_eq!(b.ticker("BSC", "USDT-BSC"), Some("USDT"));
    b.insert(
        "USDT-BSC",
        "BSC",
        NETWORK,
        &json!({"coin":"USDT-BSC","name":"Other coin"}),
    );
    assert_eq!(b.ticker(NETWORK, "USDT-BSC"), None);
    assert_eq!(b.ticker("BSC", "USDT-BSC"), None);
    let mut b = book();
    b.insert(
        "USDT",
        "BSC",
        NETWORK,
        &json!({"coin":"USDT","name":"USDT-BSC","contract":"another contract"}),
    );
    assert_eq!(b.ticker(NETWORK, "USDT-BSC"), None);
}

#[test]
fn only_fee_of_the_identified_jagent_return_is_in_usdt() {
    let b = book();
    let created = Transfer {
        venue: VenueId("mexc".into()),
        venue_ref: "accepted-id".into(),
        asset: Asset {
            ticker: "USDT".into(),
            contract: Some(CONTRACT.parse().unwrap()),
            chain: None,
        },
        network: "BSC".into(),
        amount: "160.5".parse().unwrap(),
        status: TransferStatus::Pending,
        tx_hash: None,
        fee: None,
        updated_at: chrono::Utc::now(),
    };
    let mut raw = Transfer {
        asset: Asset {
            ticker: "USDT-BSC".into(),
            contract: None,
            chain: None,
        },
        network: NETWORK.into(),
        status: TransferStatus::Sent,
        tx_hash: Some("returned-tx".into()),
        fee: Some(Cost::new(
            CostKind::WithdrawFee,
            Decimal::new(1, 2),
            "USDT-BSC",
            false,
        )),
        ..created.clone()
    };
    for _ in 0..2 {
        let mut sent = raw.clone();
        b.withdrawal_fee(&created, &mut sent);
        assert_eq!(sent.asset.ticker, "USDT-BSC");
        let fee = sent.fee.unwrap();
        assert_eq!(fee.asset, "USDT");
        assert_eq!(
            fee.amount,
            created.amount - "160.49".parse::<Decimal>().unwrap()
        );
    }
    let mut omitted = raw.clone();
    omitted.network.clear();
    b.withdrawal_fee(&created, &mut omitted);
    assert_eq!(omitted.fee.as_ref().unwrap().asset, "USDT");
    let mut amb = book();
    amb.insert(
        "USDT",
        "ARB",
        "Arbitrum",
        &json!({"coin":"USDT","name":"USDT-BSC","contract":"another contract"}),
    );
    let mut unknown = raw.clone();
    unknown.network.clear();
    amb.withdrawal_fee(&created, &mut unknown);
    assert_eq!(unknown.fee.as_ref().unwrap().asset, "USDT-BSC");
    let mut bad_network = raw.clone();
    bad_network.network = "Wrong Network".into();
    b.withdrawal_fee(&created, &mut bad_network);
    assert_eq!(bad_network.fee.as_ref().unwrap().asset, "USDT-BSC");
    let mut foreign = created.clone();
    foreign.network = "ARB".into();
    b.withdrawal_fee(&foreign, &mut raw);
    foreign = created.clone();
    foreign.venue_ref = "other-id".into();
    b.withdrawal_fee(&foreign, &mut raw);
    foreign = created.clone();
    foreign.asset.contract = Some(alloy_primitives::Address::ZERO);
    b.withdrawal_fee(&foreign, &mut raw);
    assert_eq!(raw.fee.as_ref().unwrap().asset, "USDT-BSC");
    raw.fee.as_mut().unwrap().asset = "MX".into();
    b.withdrawal_fee(&created, &mut raw);
    assert_eq!(raw.fee.unwrap().asset, "MX");
}
