//! На Base подтверждаем только оплату конкретной кассы и отдельно доступ адреса к торговой паре.
use alloy_primitives::keccak256;
use flate2::read::GzDecoder;
use serde_json::Value;

fn compressed(data: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(data)).unwrap()
}

#[test]
fn paid_kyber_call_survived_its_ancestors_and_matches_wallet_transfers() {
    let history = compressed(include_bytes!(
        "data/first_run_dex_2026-11-26/kaliber_base_vaults.json.gz"
    ));
    let captures = compressed(include_bytes!(
        "data/first_run_dex_2026-11-26/kaliber_base_access.json.gz"
    ));
    let sample = captures["witness"].as_str().unwrap();
    let receipt = &history["windows"][1]["receipts"][sample];
    let frames = captures["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 2);
    let surviving: Vec<_> = frames.iter().filter(|x| x["committed"] == true).collect();
    let [fill] = surviving.as_slice() else {
        panic!("ожидалась одна зафиксированная выплата")
    };
    assert_eq!(fill["cash"], receipt["cash"]);
    assert_eq!(fill["sender"], receipt["trader"]);
    let transfer = |token: &str, from: &Value, to: &Value, qty: u64| {
        receipt["logs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| {
                l["token"] == token
                    && &l["sender"] == from
                    && &l["recipient"] == to
                    && l["amount"] == qty
            })
            .count()
    };
    let usdc = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
    let aapl = captures["token"][0].as_str().unwrap();
    assert_eq!(
        transfer(usdc, &receipt["trader"], &receipt["cash"], 500250000),
        1
    );
    assert_eq!(
        transfer(aapl, &receipt["cash"], &receipt["trader"], 148604777),
        1
    );
    assert_eq!(
        captures["history"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["count"].as_u64().unwrap())
            .sum::<u64>(),
        294
    );
}

#[test]
fn canonical_timestamp_does_not_authorize_an_unlisted_payer() {
    let obs = compressed(include_bytes!(
        "data/first_run_dex_2026-11-26/kaliber_base_canonical.json.gz"
    ));
    let rows = obs["results"].as_array().unwrap();
    let error = |signature: &[u8]| {
        format!(
            "0x{}",
            alloy_primitives::hex::encode(&keccak256(signature)[..4])
        )
    };
    let whitelist = error(b"WhitelistOnlyPair()");
    let missing = error(b"PairDoesNotExist()");
    let mut historic_original = false;
    for r in rows {
        let actual = r["response"].as_str().unwrap();
        assert!(r["funding"].as_u64().unwrap() >= r["quantity"].as_u64().unwrap());
        assert_eq!(
            (
                r["ok"].as_bool(),
                r["spent"].as_u64(),
                r["received"].as_u64()
            ),
            (Some(false), Some(0), Some(0))
        );
        if r["cash"] == "0x5ebcee186821704aeac480d51b539ba4ede1fdf9" {
            assert_eq!(actual, whitelist);
            historic_original |= r["block"] == 52314933 && r["quantity"] == 500250000;
        } else {
            assert_eq!(r["cash"], "0xf639cf213b63f7e77d699ff686d591c0ba55fc63");
            assert_eq!(actual, missing);
        }
    }
    assert!(historic_original);
    assert_eq!(rows.len(), 24);
}
