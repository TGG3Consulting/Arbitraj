//! Денежные границы V5 и native EtherVista по архивным дельтам канонического блока.
use flate2::read::GzDecoder;
use serde_json::Value;

fn amount(v: &Value) -> i128 {
    v.as_str().unwrap().parse().unwrap()
}
fn delta(row: &Value, key: &str) -> i128 {
    amount(&row["after"][key]) - amount(&row["before"][key])
}

fn round_trip(row: &Value, cash: &str) {
    assert_eq!(row["path"][0], cash);
    assert_eq!(
        row["native"].as_array().unwrap().len(),
        if row["path"].as_array().unwrap().len() == 3 {
            2
        } else {
            0
        }
    );
    for v in row["cases"].as_array().unwrap() {
        let input = amount(&v["cash_amount"]);
        let bought = amount(&v["buy"]["net"]);
        let payout = amount(&v["sell"]["net"]);
        assert!(matches!(input, 50_000_000 | 75_000_000));
        assert!(bought > 0 && payout > 0);
        assert_eq!(amount(&v["sell"]["amount"]), bought);
        for (name, spend, debit, gain, credit) in [
            ("buy", "cash", input, "coin", bought),
            ("sell", "coin", bought, "cash", payout),
            ("router_dust_sell", "coin", bought, "cash", payout),
        ] {
            let leg = &v[name];
            let ok = &leg["exact"];
            assert_eq!(ok["status"], "0x1");
            assert_eq!((delta(ok, spend), delta(ok, gain)), (-debit, credit));
            for key in ["reserve", "module_native", "router_weth", "module_approval"] {
                assert_eq!(delta(ok, key), 0);
            }
            assert_eq!(
                amount(&ok["before"]["reserve"]),
                if name == "buy" {
                    0
                } else {
                    amount(&row["reserve_wei"])
                }
            );
            let no = &leg["plus_one"];
            assert_eq!(no["status"], "0x0");
            assert_eq!((delta(no, spend), delta(no, gain)), (0, 0));
        }
        assert_eq!(v["insufficient_reserve"]["status"], "0x0");
        assert!(
            amount(&v["router_dust_sell"]["exact"]["before"]["router_weth"])
                > amount(&v["sell"]["exact"]["before"]["router_weth"])
        );
    }
    assert_eq!(row["unauthorized_withdrawal"]["status"], "0x0");
    assert_eq!(
        delta(&row["owner_withdrawal"], "module_native"),
        -amount(&row["reserve_wei"])
    );
    for rec in row["native"].as_array().unwrap() {
        let buy = &rec["buy_module"];
        assert_eq!(buy["settled"][0]["spent"], rec["budget"]);
        assert_eq!(delta(buy, "coin"), delta(&rec["buy_router"], "coin"));
        assert_eq!(rec["buy_min_plus_one"]["status"], "0x0");
        let net = amount(&rec["expected_sell_net"]);
        let sell = &rec["sell_module"];
        if net <= 0 {
            assert_eq!(sell["status"], "0x0");
        } else {
            assert_eq!(sell["status"], "0x1");
            let event = &sell["settled"][0];
            assert_eq!(amount(&event["received"]), net);
            assert_eq!(event["fee"], rec["sell_fee"]);
            assert_eq!(delta(sell, "payee_native"), net + amount(&event["fee"]));
            assert_eq!(sell["settled"], rec["sell_dust"]["settled"]);
            assert!(
                rec["native_guards"]
                    .as_object()
                    .unwrap()
                    .values()
                    .all(|v| v["status"] == "0x0")
            );
        }
    }
}

#[test]
fn base_usdc_and_bridged_usdt_settle_in_the_original_wallet_currency() {
    let bytes = &include_bytes!("data/first_run_dex_2026-11-26/ethervista_v5_base.json.gz")[..];
    let rows: Vec<Value> = serde_json::from_reader(GzDecoder::new(bytes)).unwrap();
    assert_eq!(rows.len(), 2);
    for (row, cash) in rows.iter().zip([
        "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
        "0xfde4c96c8593536e31f229ea8f37b2ada2699bb2",
    ]) {
        assert_eq!(row["chain"], 8453);
        round_trip(row, cash);
    }
}
