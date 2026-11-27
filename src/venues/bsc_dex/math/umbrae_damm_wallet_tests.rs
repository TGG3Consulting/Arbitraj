//! Проверка каждой ставки и каждого остатка на реальных ответах Base вокруг одной операции.
use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;

fn physical_state(frame: &Value, start: usize, tokens: [Address; 2]) -> UmbraeDammState {
    let names = frame["names"].as_array().unwrap();
    let word = |name: &str, index: usize| {
        let offset = names.iter().position(|s| s.as_str() == Some(name)).unwrap();
        let v = &frame["results"][start + offset];
        assert_eq!(v["status"], "0x1");
        let hex = v["returnData"]
            .as_str()
            .unwrap()
            .strip_prefix("0x")
            .unwrap();
        U256::from_str_radix(&hex[64 * index..64 * (index + 1)], 16).unwrap()
    };
    let get = |name| word(name, 0);
    let fee_token = get("feeToken").to_be_bytes::<32>();
    let fee_side = tokens
        .iter()
        .position(|a| a.as_slice() == &fee_token[12..])
        .unwrap();
    let state = UmbraeDammState {
        tokens,
        reserves: [word("getReserves", 0), word("getReserves", 1)],
        balances: [get("balanceX"), get("balanceY")],
        fee_side,
        base_fee: get("baseFeeBps").to(),
        max_fee: get("maxFeeBps").to(),
        protocol_share: get("protocolFeePct").to(),
        volatility: get("volatilityAccumulator").to(),
        last_price: get("lastPrice"),
        lp_supply: get("totalSupply"),
        protocol_fees: get("protocolFeesAccumulated"),
        lp_fees: get("lpFeesAccumulated"),
        lp_index: get("lpFeeIndex"),
    };
    assert_eq!(rate(&state), Some(get("currentFeeBps").to()));
    state
}

#[test]
fn canonical_swaps_update_both_balances_fee_liabilities_price_and_lp_index() {
    let archive: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/umbrae_damm_base_wallets.json.gz"
        )[..],
    ))
    .unwrap();
    let directory: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/umbrae_base_directory.json"
    ))
    .unwrap();
    assert_eq!(archive["selector"], directory["selector"]);
    let mut legs = 0;
    for example in archive["examples"].as_array().unwrap() {
        let market = directory["markets"]["damm"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["pool"] == example["pool"])
            .unwrap();
        let address = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let tokens = [address(&market["tokens"][0]), address(&market["tokens"][1])];
        let cash = address(&example["cash"]);
        let other = *tokens.iter().find(|t| **t != cash).unwrap();
        for case in example["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["mode"] == "native")
        {
            let mut prior = None;
            for (side, input) in [("buy", cash), ("sell", other)] {
                let leg = &case[side];
                let frame = &leg["transition"];
                let first = frame["first"].as_u64().unwrap() as usize;
                let before = physical_state(frame, first, tokens);
                if let Some(s) = prior {
                    assert_eq!(before, s);
                }
                let paid = leg["paid"].as_str().unwrap().parse().unwrap();
                let received = leg["net"].as_str().unwrap().parse().unwrap();
                let f = fill(&before, input, true, paid).unwrap();
                let count = frame["names"].as_array().unwrap().len();
                let after = physical_state(frame, first + count + 1, tokens);
                assert_eq!(
                    f.swapped,
                    Swapped {
                        amount_in: paid,
                        amount_out: received,
                        complete: true
                    }
                );
                assert_eq!(
                    f.fee,
                    (after.protocol_fees - before.protocol_fees) + (after.lp_fees - before.lp_fees)
                );
                assert_eq!(f.after, after);
                let minimal = fill(&before, input, false, received).unwrap();
                assert!(
                    minimal.swapped.amount_in <= paid && minimal.swapped.amount_out >= received
                );
                prior = Some(after);
                legs += 1;
            }
        }
    }
    assert_eq!(legs, 4);
}
