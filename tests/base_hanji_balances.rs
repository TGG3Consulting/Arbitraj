//! Сверяет глубину Hanji с исполнением в прокси того же рынка и изменением балансов.
use serde_json::Value;

fn n(value: &Value) -> u128 {
    u128::from(value.as_u64().unwrap())
}

fn fee(value: u128, rate: u128) -> u128 {
    (value * rate).div_ceil(10u128.pow(18))
}

#[test]
fn hanji_base_quotes_and_net_cash_match_simulated_balances() {
    let data: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/hanji_base.json"
    ))
    .unwrap();
    assert_eq!(data["chain"], 8453);
    let networks: Value =
        serde_json::from_str(include_str!("data/base_ethereum_networks.json")).unwrap();
    assert_eq!(data["hash"], networks["base"]["hash"]);
    let mut paired_fills = 0;
    let mut changed_bids = 0;
    let mut empty = 0;
    for market in data["markets"].as_array().unwrap() {
        let fills = market["fills"].as_array().unwrap();
        if fills.is_empty() {
            assert!(market["asks"].as_array().unwrap().is_empty());
            assert!(market["bids"].as_array().unwrap().is_empty());
            empty += 1;
            continue;
        }
        let x = n(&market["scale"][0]);
        let y = n(&market["scale"][1]);
        let rate = n(&market["fee_rates"][1]) + n(&market["fee_rates"][3]);
        for f in fills {
            let ask = n(&market["asks"][0][0]);
            let shares = n(&f["received_x"]) / x;
            let spent = |qty| (qty * ask + fee(qty * ask, rate)) * y;
            assert!(shares > 0 && shares < n(&market["asks"][0][1]));
            assert_eq!(shares * x, n(&f["received_x"]));
            assert_eq!(spent(shares), n(&f["debited_y"]));
            assert!(
                n(&f["debited_y"]) <= n(&f["budget_y"]) && spent(shares + 1) > n(&f["budget_y"])
            );
            let bid = n(&f["bid_after"][0]);
            assert_eq!(f["debited_x"], f["received_x"]);
            let gross = bid * shares;
            assert_eq!((gross - fee(gross, rate)) * y, n(&f["received_y"]));
            assert_eq!(f["remaining_x"], 0);
            assert_eq!(f["expired_revert"], true);
            if n(&f["bid_before"][0]) > bid {
                assert_eq!(f["sell_at_initial_bid"][1], 0);
                changed_bids += 1;
            }
            paired_fills += 1;
        }
    }
    assert_eq!((paired_fills, changed_bids, empty), (14, 8, 1));
}
