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

// Сальдо только по ERC20 конкретного адреса. Плата за газ находится в ETH.
fn wallet_transfers(events: &Value, token: &str, owner: &str) -> (u128, u128) {
    let topic = format!("0x{:0>64}", &owner[2..]);
    let mut debited = 0;
    let mut credited = 0;
    for log in events.as_array().unwrap() {
        let topics = log["topics"].as_array().unwrap();
        if log["address"] != token
            || topics.len() != 3
            || topics[0] != "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
        {
            continue;
        }
        let amount = u128::from_str_radix(&log["data"].as_str().unwrap()[2..], 16).unwrap();
        if topics[1].as_str() == Some(topic.as_str()) {
            debited += amount;
        }
        if topics[2].as_str() == Some(topic.as_str()) {
            credited += amount;
        }
    }
    (debited, credited)
}

#[test]
fn usdt_round_trips_account_for_both_v3_pools_and_the_investors_franc_change() {
    let routes = inflate(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/aktionariat_ethereum_usdt.json.gz"
    ));
    let native = inflate(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/aktionariat_ethereum_native.json.gz"
    ));
    assert_eq!(routes["selector"], native["selector"]);
    let owner = routes["owner"].as_str().unwrap();
    let usdt = "0xdac17f958d2ee523a2206206994597c13d831ec7";
    let mut tested = std::collections::BTreeSet::new();
    for r in routes["controls"].as_array().unwrap() {
        let read = |property: &str| r[property].as_str().unwrap().parse::<u128>().unwrap();
        let units = u128::from(r["units"].as_u64().unwrap());
        let cash = r["cash"].as_str().unwrap();
        let share = r["share"].as_str().unwrap();
        let broker = r["broker"].as_str().unwrap();
        let [buy, sell] =
            <&[Value; 2]>::try_from(r["logs"].as_array().unwrap().as_slice()).unwrap();
        assert_eq!(wallet_transfers(buy, usdt, owner), (read("budget"), 0));
        assert_eq!(wallet_transfers(sell, usdt, owner), (0, read("returned")));
        assert_eq!(wallet_transfers(buy, share, owner), (0, units));
        assert_eq!(wallet_transfers(sell, share, owner), (units, 0));
        assert_eq!(wallet_transfers(buy, cash, owner), (0, read("cash_dust")));
        assert_eq!(wallet_transfers(sell, cash, owner), (0, 0));
        let baseline = native["controls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["market"] == broker && n["units"] == r["units"] && n["mode"] == "hub")
            .unwrap();
        let sell_net = baseline["sells"]["direct"]["net_cash"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            wallet_transfers(sell, cash, routes["module"].as_str().unwrap()),
            (sell_net, sell_net)
        );
        for pool in r["pools"].as_array().unwrap() {
            for logs in [buy, sell] {
                let swaps = logs.as_array().unwrap().iter().filter(|e| e["address"]==*pool
                    && e["topics"][0]=="0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67").count();
                assert_eq!(swaps, 1);
            }
        }
        assert_eq!(r["refusals"][0]["code"], 3);
        assert_eq!(r["refusals"][1]["code"], 3);
        assert!(tested.insert((cash, units)));
    }
    assert_eq!(tested.len(), 4);
}
