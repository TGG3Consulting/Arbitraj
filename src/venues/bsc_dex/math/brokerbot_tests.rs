use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;
fn inflate(raw: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(raw)).unwrap()
}
fn value(o: &Value, k: &str) -> U256 {
    o[k].as_str().unwrap().parse().unwrap()
}
fn address(o: &Value, k: &str) -> Address {
    o[k].as_str().unwrap().parse().unwrap()
}

#[test]
fn curves_match_paid_shares_and_seller_cash_after_a_real_purchase() {
    let d = inflate(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/aktionariat_ethereum_directory.json.gz"
    ));
    let n = inflate(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/aktionariat_ethereum_native.json.gz"
    ));
    let curves: Value = serde_json::from_slice(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/aktionariat_ethereum_curves.json"
    ))
    .unwrap();
    assert_eq!(curves["selector"], n["selector"]);
    let mut markets = 0;
    let mut paid_sales = std::collections::BTreeSet::new();
    for r in curves["rows"].as_array().unwrap() {
        let key = r["broker"].as_str().unwrap();
        let m = &d["markets"][key];
        let keys: Vec<_> = r["setting"].as_str().unwrap().split_whitespace().collect();
        let flags = m["settings"].as_u64().unwrap();
        let s = BrokerbotState {
            broker: address(r, "broker"),
            cash: address(m, "cash"),
            share: address(m, "share"),
            hub: address(m, "hub"),
            registered: m["registered_pair"].is_array(),
            buying: flags & 1 != 0,
            selling: flags & 2 != 0,
            price: value(r, "price"),
            base_price: value(r, "base_price"),
            increment: value(r, "increment"),
            cash_balance: value(m, "cash_balance"),
            share_balance: value(m, "shares"),
            supply: value(r, "supply"),
            average_first: keys[7] == "average",
            license_bps: keys[8].parse().unwrap(),
            drift: r["drift"].as_bool().unwrap(),
        };
        assert!(s.swap(s.share, true, U256::ONE).is_none());
        assert_eq!(s.gross(true, U256::ONE), Some(value(m, "buy_one")));
        for row in n["controls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["market"] == key && v["buy_received"].is_string())
        {
            let qty = value(row, "buy_received");
            let cost = value(row, "buy_spent");
            let buy = s.swap(s.cash, true, cost).unwrap();
            assert_eq!((buy.amount_in, buy.amount_out), (cost, qty), "{key}");
            let requested = s.swap(s.cash, false, qty).unwrap();
            assert_eq!((requested.amount_in, requested.amount_out), (cost, qty));
            let after = s.after(s.cash, cost).unwrap();
            let sell = &row["sells"]["direct"];
            let net = value(sell, "net_cash");
            if net.is_zero() {
                assert!(after.swap(s.share, true, qty).is_none());
                continue;
            }
            paid_sales.insert(key);
            assert_eq!(
                after.gross(false, qty),
                Some(value(row, "sell_price_after_buy"))
            );
            assert_eq!(after.swap(s.share, true, qty).unwrap().amount_out, net);
            assert_eq!(after.swap(s.share, false, net).unwrap().amount_in, qty);
        }
        markets += 1;
    }
    assert_eq!((markets, paid_sales.len()), (10, 4));
}
#[test]
fn odd_increment_changes_the_first_sufficient_sale() {
    let u = U256::from;
    let s = BrokerbotState {
        broker: Address::ZERO,
        hub: Address::ZERO,
        cash: Address::repeat_byte(1),
        share: Address::repeat_byte(2),
        registered: true,
        buying: true,
        selling: true,
        price: u(4u64),
        base_price: u(4),
        increment: U256::ONE,
        cash_balance: u(6),
        share_balance: u(4),
        supply: u(8),
        average_first: true,
        license_bps: 0,
        drift: false,
    };
    assert_eq!(s.sell(u(3)), Some(u(6)));
    assert_eq!(s.sell(u(4)), Some(u(4)));
    assert_eq!(s.swap(s.share, false, u(6)).unwrap().amount_in, u(3));
    assert_eq!(s.gross(true, u(2)), Some(u(8)));
    let updated = BrokerbotState {
        average_first: false,
        ..s
    };
    assert_eq!(updated.gross(true, u(2)), Some(u(9)));
}
