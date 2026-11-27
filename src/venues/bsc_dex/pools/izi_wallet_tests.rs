//! BitSwap Ethereum. Декодер котировок сохраняет исполнимое округление и отбрасывает исчерпанную ступень.
use crate::venues::bsc_dex::quote::virtual_quoter;
use alloy::sol_types::SolValue;
use alloy_primitives::U256;
use flate2::read::GzDecoder;
use serde_json::Value;

fn number(value: &Value) -> U256 {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn stablecoin_wallets_separate_callback_change_from_an_exhausted_pool() {
    let bytes = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/bitswap_ethereum_wallets.json.gz"
    );
    let fixture: Value = serde_json::from_reader(GzDecoder::new(&bytes[..])).unwrap();
    let rows = fixture["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 12);
    let mut edges = 0;
    let mut change = 0;
    for row in rows {
        let budget = number(&row["authorized"]);
        let used = number(&row["quote_used"]);
        let spent = number(&row["spent"]);
        let credit = number(&row["received"]);
        assert_eq!(spent, number(&row["before"][0]) - number(&row["after"][0]));
        assert_eq!(credit, number(&row["after"][1]) - number(&row["before"][1]));
        assert_eq!(row["before"][2], row["after"][2]);
        assert_eq!(row["before"][3], row["after"][3]);
        assert!(row["strict"].is_object() && row["expired"].is_object());
        let quoted = virtual_quoter::decode(&(used, credit).abi_encode(), budget).unwrap();
        if used < budget {
            edges += 1;
            assert!(quoted.is_err());
            assert_eq!(used, spent);
        } else {
            assert_eq!(quoted, Ok(credit));
            assert!(spent <= used);
            change += usize::from(spent < used);
        }
    }
    assert_eq!((edges, change), (5, 2));
}
