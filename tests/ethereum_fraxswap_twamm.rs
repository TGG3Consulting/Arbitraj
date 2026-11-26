//! Сверяет выплаты с каноническими резервами Fraxswap и показывает границу обычного V2-шага V5.
use alloy_primitives::U256;
use arb::venues::bsc_dex::{
    math::v2,
    pools::{exec_kind::ExecKind, families, per_block},
};
use rust_decimal::Decimal;
use serde_json::Value;

fn number(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn projected_reserves_match_observed_wallet_output_when_generic_v2_reverts() {
    let registry: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/fraxswap_ethereum.json"
    ))
    .unwrap();
    let actual: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/fraxswap_v5_comparison.json"
    ))
    .unwrap();
    assert_eq!(registry["selector"], actual["selector"]);
    let markets = registry["markets"].as_array().unwrap();
    assert_eq!(markets.len(), 30);
    let nonzero = |x: &Value| number(x) > U256::ZERO;
    assert_eq!(
        markets
            .iter()
            .filter(|m| nonzero(&m["twamm_reserves"][3]) || nonzero(&m["twamm_reserves"][4]))
            .count(),
        15
    );
    assert_eq!(
        markets
            .iter()
            .filter(|m| nonzero(&m["twamm_state"][0]) || nonzero(&m["twamm_state"][1]))
            .count(),
        1
    );
    let now = actual["timestamp"].as_u64().unwrap().to_string();
    let mut reverted = 0;
    let mut generic_success = 0;
    for c in actual["cases"].as_array().unwrap() {
        let m = markets.iter().find(|m| m["pool"] == c["pool"]).unwrap();
        let i = m["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .position(|t| t == &c["token_in"])
            .unwrap();
        let r = &m["projection"][&now]["decoded"];
        let bps = m["twamm_reserves"][5]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap();
        let fee = v2::fee_ratio(Decimal::new(bps, 2)).unwrap();
        let q =
            v2::amount_out(number(&c["amount"]), number(&r[i]), number(&r[1 - i]), fee).unwrap();
        assert_eq!(number(&c["native"]["received"]), q);
        assert_eq!(c["native"]["spent"], c["amount"]);
        assert_eq!(c["strict"]["status"], "0x0");
        assert_eq!(number(&c["strict"]["spent"]), U256::ZERO);
        if c["v5"]["status"] == "0x1" {
            assert_eq!(number(&c["v5"]["received"]), q);
            generic_success += 1;
        } else {
            assert_eq!(c["v5"]["error"]["data"], "0x19886c40");
            assert_eq!(number(&c["v5"]["received"]), U256::ZERO);
            assert_eq!(number(&c["v5"]["spent"]), U256::ZERO);
            reverted += 1;
        }
    }
    assert_eq!((reverted, generic_success), (8, 2));
    let observe = families::make("market@ethereum", "fraxswap_twamm").unwrap();
    let active = families::make("market@ethereum", "fraxswap_twamm_v5").unwrap();
    assert!(per_block::needs(observe) && per_block::needs(active));
    assert_eq!(observe.executor_kind, None);
    assert_eq!(active.executor_kind, Some(ExecKind::Basin));
}
