//! Сверка на архивных маршрутах и реальных записях OasisSwap и BSC.
use super::*;
use alloy::sol_types::SolCall;
use serde_json::Value;

#[test]
fn old_bsc_hop_has_identical_bytes_and_rebate_selection_survives_restart() {
    use crate::core::types::{RouteHop, RouterFeeMode};
    let raw: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/bsc_units_2026_11_24/signal_quotes.json"
    ))
    .unwrap();
    let legacy = &raw["sell_quote"]["route"]["legs"][0]["hops"][0];
    let mut hop: RouteHop = serde_json::from_value(legacy.clone()).unwrap();
    assert!(hop.fee_mode.is_none());
    let expected = format!(
        "{{\"family\":{},\"kind\":{},\"pool\":{}}}",
        legacy["family"], legacy["kind"], legacy["pool"]
    );
    assert_eq!(serde_json::to_string(&hop).unwrap(), expected);
    assert_eq!(chosen_fee(None, false, true), Some(false));
    hop.fee_mode = Some(RouterFeeMode::Standard);
    let saved: RouteHop = serde_json::from_slice(&serde_json::to_vec(&hop).unwrap()).unwrap();
    assert_eq!(chosen_fee(saved.fee_mode, false, true), Some(false));
    hop.fee_mode = Some(RouterFeeMode::Rebate);
    let saved: RouteHop = serde_json::from_slice(&serde_json::to_vec(&hop).unwrap()).unwrap();
    assert_eq!(chosen_fee(saved.fee_mode, false, true), Some(true));
    assert_eq!(chosen_fee(saved.fee_mode, false, false), None);
    assert_eq!(chosen_fee(saved.fee_mode, true, true), None);
}

/// Записанные покупки, продажи и отрицательные контроли на том же размере и плательщике.
#[test]
fn funded_graph_wallet_outcomes_separate_pools_sizes_and_minimums() {
    let rows: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/oasisswap_v5_base.json"
    ))
    .unwrap();
    let native: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/oasisswap_base.json"
    ))
    .unwrap();
    assert_eq!(rows["selector"], native["selector"]);
    assert_eq!(rows["payer"], native["payer"]);
    let payer = rows["payer"].as_str().unwrap().parse().unwrap();
    let number = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let mut accepted = 0;
    let mut failed = 0;
    for row in rows["cases"].as_array().unwrap() {
        let input = row["asset"].as_str().unwrap().parse().unwrap();
        let output = row["token"].as_str().unwrap().parse().unwrap();
        let size = number(&row["size"]);
        let minimum = number(&row["buy"]["accepted"]["received"]);
        let call = swap_data(input, output, size, minimum, payer, U256::MAX, false);
        let decoded =
            IOasisRouter::swapExactTokensForTokensSupportingFeeOnTransferTokensCall::abi_decode(
                &call,
            )
            .unwrap();
        assert_eq!(
            (
                decoded.amountIn,
                decoded.amountOutMin,
                decoded.to,
                decoded.path,
                decoded.useRebate
            ),
            (size, minimum, payer, vec![input, output], false)
        );
        let strict = swap_data(
            input,
            output,
            size,
            minimum + U256::ONE,
            payer,
            U256::MAX,
            false,
        );
        assert_ne!(call, strict);
        let with_rebate = swap_data(input, output, size, minimum, payer, U256::MAX, true);
        assert_ne!(call, with_rebate);
        assert_ne!(
            call[..4],
            super::super::fork_swap::swap_data(size, minimum, &[input, output], payer, U256::MAX)
                [..4]
        );
        for side in ["buy", "sell"] {
            let snap = &row[side];
            if let Some(ok) = snap.get("accepted") {
                let rejected = &snap["strict"];
                assert_eq!(ok["status"], "0x1");
                assert_eq!(
                    &ok["spent"],
                    if side == "buy" {
                        &row["size"]
                    } else {
                        &row["buy"]["accepted"]["received"]
                    }
                );
                assert_eq!(ok["received"], ok["floor"]);
                assert_eq!(
                    number(&rejected["floor"]),
                    number(&ok["received"]) + U256::ONE
                );
                assert_eq!(rejected["status"], "0x0");
                assert_eq!(rejected["received"], "0");
                assert_eq!(rejected["spent"], "0");
                accepted += 1;
            } else {
                let low = &snap["minimum_one"];
                assert_eq!(side, "sell");
                assert_eq!(low["floor"], "1");
                assert!(number(&low["native_quote"]) > U256::ZERO);
                assert_eq!(low["status"], "0x0");
                assert_eq!(low["spent"], "0");
                assert_eq!(low["received"], "0");
                failed += 1;
            }
        }
    }
    assert_eq!((accepted, failed), (6, 2));
}
