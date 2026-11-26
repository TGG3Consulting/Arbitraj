//! Сверяет глубину Hanji с исполнением в прокси того же рынка и изменением балансов.
use alloy_primitives::U256;
use arb::venues::bsc_dex::hanji::{Level, Side, Units};
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
        let units = Units {
            x: U256::from(x),
            y: U256::from(y),
            fee_rate: rate,
        };
        let asks: Vec<_> = market["asks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| Level {
                price: n(&p[0]),
                shares: n(&p[1]),
            })
            .collect();
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
            let buy = units
                .fill(&asks, Side::BuyX, U256::from(n(&f["budget_y"])))
                .unwrap();
            assert_eq!(buy.spent, U256::from(n(&f["debited_y"])));
            let exact = units
                .exact_quantity(&asks, Side::BuyX, buy.received, true)
                .unwrap();
            assert_eq!((exact.spent, exact.received), (buy.spent, buy.received));
            assert_eq!(buy.received, U256::from(n(&f["received_x"])));
            let fresh_bid = Level {
                price: n(&f["bid_after"][0]),
                shares: n(&f["bid_after"][1]),
            };
            let sell = units.fill(&[fresh_bid], Side::SellX, buy.received).unwrap();
            assert_eq!(sell.unspent, U256::ZERO);
            assert_eq!(sell.received, U256::from(n(&f["received_y"])));
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

#[test]
fn hanji_crosses_two_price_levels_and_reconciles_both_fees() {
    let data: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/hanji_depth_base.json"
    ))
    .unwrap();
    let mut paths = 0;
    for market in data["markets"].as_array().unwrap() {
        let units = Units {
            x: U256::from(n(&market["scale"][0])),
            y: U256::from(n(&market["scale"][1])),
            fee_rate: n(&market["fee_rates"][1]) + n(&market["fee_rates"][3]),
        };
        let levels = |arr: &Value| {
            arr.as_array()
                .unwrap()
                .iter()
                .take(2)
                .map(|v| Level {
                    price: n(&v[0]),
                    shares: n(&v[1]),
                })
                .collect::<Vec<_>>()
        };
        for f in market["fills"].as_array().unwrap() {
            let buy = units
                .fill(
                    &levels(&market["asks"]),
                    Side::BuyX,
                    U256::from(n(&f["budget_y"])),
                )
                .unwrap();
            assert_eq!(buy.last_price, n(&market["asks"][1][0]));
            assert_eq!(buy.received, U256::from(n(&f["received_x"])));
            assert_eq!(buy.spent, U256::from(n(&f["debited_y"])));
            let sell = units
                .fill(&levels(&f["fresh_bids"]), Side::SellX, buy.received)
                .unwrap();
            assert_eq!(sell.last_price, n(&f["fresh_bids"][1][0]));
            assert_eq!(sell.received, U256::from(n(&f["received_y"])));
            assert_eq!(sell.spent, buy.received);
            let recovered = units
                .exact_quantity(&levels(&f["fresh_bids"]), Side::SellX, sell.received, true)
                .unwrap();
            assert_eq!((recovered.spent, recovered.received), (sell.spent, sell.received));
            assert_eq!(sell.unspent, U256::ZERO);
            paths += 1;
        }
    }
    assert_eq!(paths, 3);
}
