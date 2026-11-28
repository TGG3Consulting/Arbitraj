//! Сопоставление quote2 с изолированными выплатами на Base для полного и частичного takeOrders4.
use super::*;
use crate::venues::bsc_dex::raindex::amount::fill_size;

#[test]
fn base_v6_quote_and_wallet_agree_at_whole_token_unit_boundaries() {
    let quotes: Value = serde_json::from_reader(GzDecoder::new(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_quotes.json.gz"
    ).as_slice())).unwrap();
    let wallets: Value = serde_json::from_reader(GzDecoder::new(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_protected_wallets.json.gz"
    ).as_slice())).unwrap();
    let edges: Value = serde_json::from_reader(GzDecoder::new(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_capacity_edges.json.gz"
    ).as_slice())).unwrap();
    assert_eq!(quotes["anchor"], wallets["selector"]);
    assert_eq!(edges["selector"], wallets["selector"]);
    assert_eq!(edges["payer"], wallets["payer"]);
    assert_eq!(edges["cases"].as_array().unwrap().len(), 7);
    let mut counts = (0, 0, 0);
    for pair in wallets["pairs"].as_array().unwrap() {
        let partial = &pair["partial_capacity"];
        if partial["stage"] != "paid" {
            continue;
        }
        let entry = quotes["entries"].as_array().unwrap().iter()
            .find(|e| e["hash"] == partial["order"]).unwrap();
        let io = entry["quotes"].as_array().unwrap().iter()
            .find(|q| q["i"] == partial["i"] && q["j"] == partial["j"]).unwrap();
        assert_eq!(quotes["book"], partial["book"]);
        assert_eq!(io["pay"], partial["pay"]);
        assert_eq!(io["receive"], partial["receive"]);
        // Во всех семи заявках оплачивается USDC (6 знаков), токен выдачи имеет 18 знаков.
        let decimals = (6, 18);
        let float = |key: &str| Quantity::float(from_value(io[key].clone()).unwrap()).unwrap();
        let cap = float("cap").cap(decimals.1).unwrap();
        let ratio = float("ratio");
        let size = |wanted, buy| fill_size(ratio, Some(cap), wanted, decimals, buy, 6);
        for case in pair["cases"].as_array().unwrap() {
            for (key, fixed_output) in [("by_output", true), ("by_debit", false)] {
                let row = &case[key];
                assert_eq!(row["order"], partial["order"]);
                assert_eq!((&row["i"], &row["j"]), (&io["i"], &io["j"]));
                assert_eq!(size(amount(row, "size"), fixed_output),
                    Some((amount(row, "net"), amount(row, "paid"), fixed_output)));
                counts.0 += 1;
            }
        }

        let paid = amount(partial, "paid");
        assert_eq!(cap, amount(partial, "net"));
        let edge = edges["cases"].as_array().unwrap().iter().find(|r|
            r["order"] == partial["order"] && r["i"] == io["i"] && r["j"] == io["j"]).unwrap();
        assert_eq!(edge["outputMax"], io["cap"]);
        assert_eq!(edge["ratio"], io["ratio"]);
        let (output, debit, fixed_receive) = size(paid, false).unwrap();
        assert_eq!((output, debit), (cap, paid));
        let order = wire::Order::decode(6, &from_value::<Bytes>(entry["order"].clone()).unwrap(),
            from_value(entry["hash"].clone()).unwrap()).unwrap();
        let action = order.take(
            io["i"].as_u64().unwrap() as usize, io["j"].as_u64().unwrap() as usize,
            if fixed_receive { output } else { debit },
            if fixed_receive { decimals.1 } else { decimals.0 },
            fixed_receive, from_value(edge["ratio"].clone()).unwrap(),
        ).unwrap();
        let previous = &edge["exactDebit"];
        let corrected = &edge["exactReceive"];
        let chosen = if fixed_receive { corrected } else { previous };
        let call: Bytes = from_value(chosen["call"]["data"].clone()).unwrap();
        let fill = wire::fillCall::abi_decode_validate(&call).unwrap();
        assert_eq!(fill.action, action);
        assert_eq!((fill.l.minimumDebit, fill.l.maximumDebit, fill.l.minimumCredit),
            (paid, paid, cap));
        assert_eq!(chosen["call"]["from"], wallets["payer"]);
        assert_eq!(chosen["call"]["to"], partial["target"]);
        assert_eq!(chosen["status"], "0x1");
        assert_eq!(previous["before"], corrected["before"]);
        assert_eq!(corrected["status"], "0x1");
        for op in [previous, corrected] {
            if op["status"] == "0x0" {
                assert!(fixed_receive);
                assert_eq!(op["after"], op["before"]);
                assert!(op["logs"].as_array().unwrap().is_empty());
            } else {
                let b = op["before"].as_array().unwrap();
                let a = op["after"].as_array().unwrap();
                let raw = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
                assert_eq!((raw(&b[0])-raw(&a[0]), raw(&a[1])-raw(&b[1])), (paid, cap));
                assert_eq!(b[2..], a[2..]);
            }
        }
        assert_eq!(previous["status"] == "0x0", fixed_receive);
        assert!(size(paid + U256::ONE, false).is_none());
        assert!(size(amount(partial, "size"), true).is_none());
        if ratio.received(paid, decimals.0, decimals.1).unwrap() > cap {
            counts.2 += 1;
        }
        counts.1 += 1;
    }
    assert_eq!(counts, (42, 7, 5));
}
