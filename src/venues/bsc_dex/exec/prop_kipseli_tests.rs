//! Проверка трёх оплаченных прямых поручений Kipseli по оригинальной calldata, событиям и переводу кассы.
use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn named_source_requires_each_funded_hop_and_its_own_venue_event() {
    let source: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/kipseli_ethereum.json.gz")
            [..],
    ))
    .unwrap();
    let base: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/propamm_router_ethereum.json.gz"
        )[..],
    ))
    .unwrap();
    let role = |key: &str| {
        source["roles"][key]["address"]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap()
    };
    let rows = source["ordinary"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for row in rows {
        let tx = &row["tx"];
        let owner = tx["from"].as_str().unwrap().parse::<Address>().unwrap();
        let input = tx["input"].as_str().unwrap().parse::<Bytes>().unwrap();
        let order = IProp::swapViaVenueV1Call::abi_decode_validate(&input).unwrap();
        let mut proof = tests::make(&order, owner, &base);
        proof.terms.via = Some([role("pool"), role("logic")]);
        proof.terms.vault = role("vault");
        assert_eq!(proof.data(), input);
        let logs: Vec<SimLog> = serde_json::from_value(row["receipt"]["logs"].clone()).unwrap();
        let logs = signed_call::logs(&logs).unwrap();
        let value = fill::moved_logs(&logs, proof.output, Some(role("vault")), Some(owner));
        assert!(value >= proof.minimum && !value.is_zero());
        assert_eq!(
            proof.outcome(&logs),
            Some((role("venue"), proof.amount, value))
        );
        for i in 0..logs.len() {
            if logs[i].address() == proof.input
                || logs[i].address() == proof.output
                || logs[i].address() == role("venue")
            {
                let mut broken = logs.clone();
                broken.remove(i);
                assert_eq!(proof.outcome(&broken), None);
            }
        }
        proof.minimum = value + U256::ONE;
        assert_eq!(proof.outcome(&logs), None);
    }
}
