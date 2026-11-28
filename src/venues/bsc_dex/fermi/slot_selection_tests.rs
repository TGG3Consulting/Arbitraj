//! Проверка фактических двух рыночных выплат и допуска кадра по архиву строителя.
use super::super::{Deployment, Frame, slot_config::same_frame};
use super::*;
use alloy::primitives::{Address, B256};
use flate2::read::GzDecoder;
use serde_json::Value;
fn fixture(data: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(data)).unwrap()
}
#[test]
fn two_real_venues_change_winner_with_price_of_gas_and_only_share_usdc() {
    let a = fixture(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/kaliber_ethereum_bound_v2.json.gz"
    ));
    let b = fixture(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/taurus_ethereum_bound_v2.json.gz"
    ));
    let stable = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
    let weth = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2";
    let sell = |j: &Value| {
        let rows = j["builders"][0]["rows"].as_array().unwrap();
        let mut found = rows.iter().filter(|r| {
            r["input"] == weth
                && r["output"] == stable
                && r["withFrame"]["paid"] == 10000000000000000u64
        });
        let r = &found.next().unwrap()["withFrame"];
        assert_eq!(r["status"], "paid");
        assert!(found.next().is_none());
        (
            Decimal::from(r["net"].as_u64().unwrap()),
            Decimal::from(r["gas"].as_u64().unwrap()),
        )
    };
    let (k, t) = (sell(&a), sell(&b));
    assert_eq!(k.0 - t.0, Decimal::from(9907));
    assert_eq!(k.1 - t.1, Decimal::from(51822));
    for (gas_in_cash_atoms, kaliber) in [(Decimal::new(1, 1), true), (Decimal::new(2, 1), false)] {
        assert_eq!(
            improves(
                Side::Sell,
                k.0 - k.1 * gas_in_cash_atoms,
                Some(t.0 - t.1 * gas_in_cash_atoms)
            ),
            kaliber
        );
    }
    let usdt = "0xdac17f958d2ee523a2206206994597c13d831ec7";
    assert!(
        !a["builders"][0]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["output"] == usdt)
    );
    assert!(
        b["builders"][0]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["input"] == weth
                && r["output"] == usdt
                && r["withFrame"]["status"] == "paid")
    );
}
#[test]
fn refreshed_slot_rejects_another_oracle_or_second_price_with_the_same_time() {
    let dep = Deployment {
        engine: Address::repeat_byte(1),
        router: Address::repeat_byte(2),
        runtime: B256::repeat_byte(3),
    };
    let f = Frame {
        deployment: dep,
        slot: 3,
        target: 4,
        timestamp: 1606824059,
        diff: std::collections::BTreeMap::from([(B256::ZERO, B256::repeat_byte(4))]),
    };
    let mut new = f.clone();
    assert!(same_frame(&f, &new));
    new.diff.insert(B256::ZERO, B256::repeat_byte(5));
    assert!(!same_frame(&f, &new));
    new = f.clone();
    new.deployment.engine = Address::repeat_byte(7);
    assert!(!same_frame(&f, &new));
}

#[test]
fn builder_names_authorize_only_one_venue_and_require_an_unambiguous_choice() {
    use super::super::{Builder, slot_config::builders};
    assert!(builders(Vec::new()).is_empty());
    assert_eq!(builders(vec!["titan".into()]), vec![Builder::Titan]);
    assert_eq!(
        builders(vec!["buildernet".into()]),
        vec![Builder::BuilderNet]
    );
    assert!(builders(vec!["old".into(), "titan".into()]).is_empty());
    assert!(builders(vec!["titan".into(), "titan".into()]).is_empty());
}

#[test]
fn funded_bop_cash_competes_with_an_actual_usdt_payout_after_gas() {
    let rows = fixture(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/bopamm_ethereum_cash_wallet.json.gz"
    ));
    let number = |v: &Value| v.as_str().unwrap().parse::<i128>().unwrap();
    let mut wins = [0; 2];
    for frame in rows["frames"].as_array().unwrap() {
        let sale = &frame["cases"][0];
        assert_eq!(
            sale["input"].as_str().unwrap().parse::<Address>().unwrap(),
            super::super::bop_binding::Identity::issued()
                .unwrap()
                .cash
                .weth
                .address
        );
        let amount = number(&sale["amount"]);
        let lambda = &sale["lambda"];
        let original = number(&lambda["received"]);
        let scored = |trial: &Value| {
            // На каждом кадре курс ETH/gas измерен этим же точным обменом WETH в физический USDT.
            let fee = trial["gas"].as_i64().unwrap() as i128 * number(&frame["gas_price_wei"]);
            let whole = number(&trial["received"]) * amount - fee * original;
            rust_decimal::Decimal::from_i128_with_scale(whole, 18)
        };
        let baseline = scored(lambda);
        for (index, key) in ["nested_module", "compound"].iter().enumerate() {
            assert_eq!(sale[*key]["outcome"], "paid");
            if improves(Side::Sell, scored(&sale[*key]), Some(baseline)) {
                wins[index] += 1;
            }
        }
    }
    assert_eq!(wins, [0, 4]);
}
