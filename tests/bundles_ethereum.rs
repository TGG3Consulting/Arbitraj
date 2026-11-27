//! Archived wallet transfers and Sync events for every registered Bundles Swap market.
use alloy_primitives::{Address, U256};
use arb::venues::bsc_dex::math::bundles::{Book, Market};
use serde_json::Value;

fn address(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn qty(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

fn state(data: &Value, obs: &Value, pool: Address) -> Market {
    let m = data["markets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| address(&m["pool"]) == pool)
        .unwrap();
    let tokens = std::array::from_fn(|i| address(&m["tokens"][i]));
    let oracle = &obs["oracles"][m["oracle"].as_str().unwrap()];
    let rate = tokens
        .iter()
        .position(|x| *x == address(&oracle["token1"]))
        .map(|i| (i, qty(&oracle["price1Average"])));
    Market {
        pool,
        tokens,
        tier: u16::try_from(m["fee"].as_u64().unwrap()).unwrap(),
        reserves: std::array::from_fn(|i| qty(&m["reserves"][i])),
        balances: std::array::from_fn(|i| qty(&m["balances"][i])),
        fee_reserve: qty(&m["fee_reserve"]),
        supply: qty(&obs["supply"][m["pool"].as_str().unwrap()]),
        oracle: rate,
    }
}

#[test]
fn both_payment_methods_match_the_wallet_and_every_affected_pool() {
    let payload = include_bytes!("data/first_run_dex_2026-11-26/bundles_ethereum_wallets.json.gz");
    let data: Value =
        serde_json::from_reader(flate2::read::GzDecoder::new(payload.as_slice())).unwrap();
    let obs: Value = serde_json::from_slice(include_bytes!(
        "data/first_run_dex_2026-11-26/bundles_ethereum_oracles.json"
    ))
    .unwrap();
    assert_eq!(data["selector"], obs["selector"]);
    let router = &data["router"];
    let fee_pair = state(&data, &obs, address(&router["oracle_pair"]));
    let cases = data["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 84);
    let signature = format!(
        "{:#x}",
        alloy_primitives::keccak256("Sync(uint112,uint112)")
    );
    for row in cases {
        assert_eq!(row["stage"], "settled");
        let pool = address(&row["pool"]);
        let input = address(&row["input"]);
        let book = Book {
            market: state(&data, &obs, pool),
            fee_pair: fee_pair.clone(),
            bun: address(&data["protocol_token"]),
            weth: address(&router["weth"]),
            router_balance: qty(&router["fee_token_balance"]),
            deposits: qty(&router["total_deposits"]),
        };
        let out = book.market.tokens[usize::from(book.market.tokens[0] == input)];
        let fill = book
            .exact_in(input, qty(&row["amount"]), row["mode"] == "separate")
            .unwrap();
        assert_eq!(fill.credit, qty(&row["quote"]));
        if row["mode"] != "separate" || book.market.tokens.contains(&book.bun) {
            let (least, _) = book
                .swap(input, false, fill.credit)
                .unwrap_or_else(|| panic!("Bundles inverse pool={pool:#x} input={input:#x} amount={} target={} mode={}",row["amount"],fill.credit,row["mode"]));
            assert!(least.amount_in <= qty(&row["amount"]));
            assert!(least.amount_out >= fill.credit);
            assert!(
                book.exact_in(input, least.amount_in - U256::ONE, false)
                    .is_none_or(|smaller| smaller.credit < fill.credit)
            );
        }

        assert_eq!(fill.credit, qty(&row["delta"][format!("{out:#x}")]));
        if !book.market.tokens.contains(&book.bun) {
            let debit = row["delta"][format!("{:#x}", book.bun)]
                .as_str()
                .unwrap()
                .trim_start_matches('-')
                .parse::<U256>()
                .unwrap();
            assert_eq!(fill.bun_spent, debit);
        }
        let mut count = 0;
        for log in row["logs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| l["topics"][0] == signature)
        {
            let site = address(&log["address"]);
            let predicted = if site == pool {
                &fill.market
            } else {
                assert_eq!(site, fee_pair.pool);
                &fill.fee_pair
            };
            let raw = log["data"].as_str().unwrap().strip_prefix("0x").unwrap();
            assert_eq!(raw.len(), 128);
            assert_eq!(
                predicted.reserves,
                [
                    U256::from_str_radix(&raw[..64], 16).unwrap(),
                    U256::from_str_radix(&raw[64..], 16).unwrap()
                ]
            );
            count += 1;
        }
        assert_eq!(count, if row["mode"] == "converted" { 2 } else { 1 });
    }
}
