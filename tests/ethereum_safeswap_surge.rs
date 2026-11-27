//! Архивные выплаты SafeSwap и Surge по канонической высоте обеих групп измерений.
use alloy_primitives::U256;
use serde_json::Value;

fn archive(bytes: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
}
fn raw(value: &Value) -> U256 {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn safeswap_pair_and_router_fees_are_independent() {
    let d = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/safeswap_ethereum_wallets.json.gz"
    ));
    let registry = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/safeswap_ethereum_directory.json.gz"
    ));
    assert_eq!(d["selector"], registry["selector"]);
    assert_eq!(d["state_root"], registry["state_root"]);
    let deps = d["deployments"].as_array().unwrap();
    assert_eq!(deps.len(), 4);
    for (index, dep) in deps.iter().enumerate() {
        let markets = dep["markets"].as_array().unwrap();
        assert_eq!(markets.len(), [8, 9, 4, 8][index]);
        let proof = &registry["deployments"][index];
        assert_eq!(proof["roles"], dep["roles"]);
        assert_eq!(proof["init"], dep["init"]);
        for (market, original) in markets.iter().zip(proof["markets"].as_array().unwrap()) {
            for key in ["pool", "tokens", "reserves", "balances", "runtime"] {
                assert_eq!(market[key], original[key]);
            }
        }
        assert_eq!(dep["creation_code"]["hash"], dep["init"]);
        if index >= 2 {
            for role in ["factory", "router"] {
                assert_ne!(
                    dep["roles"][role]["address"],
                    dep["implementations"][role]["address"]
                );
                assert!(
                    dep["implementations"][role]["runtime"]
                        .as_str()
                        .is_some_and(|h| h.len() == 66)
                );
            }
        }
        if index == 3 {
            assert_eq!(dep["access"]["router_version"], 1);
            let mut legs = 0;
            for round in markets
                .iter()
                .flat_map(|m| m["paid_public"].as_array().unwrap())
            {
                for side in ["buy", "sell"] {
                    let leg = &round[side];
                    let Some(ok) = leg.get("exact") else { continue };
                    assert_eq!(ok["transaction"]["to"], dep["access"]["trade"]["address"]);
                    assert_eq!(ok["status"], "0x1");
                    let before = ok["before"].as_array().unwrap();
                    let after = ok["after"].as_array().unwrap();
                    assert_eq!(raw(&before[0]) - raw(&after[0]), raw(&ok["spent"]));
                    assert_eq!(raw(&after[1]) - raw(&before[1]), raw(&ok["received"]));
                    assert_eq!(raw(&before[2]) - raw(&after[2]), raw(&ok["fee"]));
                    assert!(raw(&ok["fee"]) > U256::ZERO);
                    assert_eq!(&before[3..], &after[3..]);
                    for refused in ["above", "expired"] {
                        assert_eq!(leg[refused]["code"], 3);
                    }
                    if side == "sell" {
                        assert_eq!(ok["spent"], round["buy"]["exact"]["received"]);
                    }
                    legs += 1;
                }
            }
            assert_eq!(legs, 4);
            continue;
        }
        assert_eq!(dep["pair_pips"], 2000);
        assert_eq!(dep["router_pips"], [2500, 2000, 2000][index]);
        let mut same_input = 0;
        for market in markets {
            let Some(rounds) = market["rounds"].as_array() else {
                continue;
            };
            for round in rounds {
                for side in ["buy", "sell"] {
                    let leg = &round[side];
                    let Some(ok) = leg.get("exact") else { continue };
                    assert_eq!(ok["status"], "0x1");
                    assert!(raw(&ok["spent"]) > U256::ZERO && raw(&ok["received"]) > U256::ZERO);
                    for refusal in ["above", "expired"] {
                        assert_eq!(leg[refusal]["status"], "0x0");
                        assert_eq!(leg[refusal]["spent"], "0");
                        assert_eq!(leg[refusal]["received"], "0");
                    }
                    if side == "sell" {
                        assert_eq!(ok["spent"], round["buy"]["exact"]["received"]);
                    }
                    if round["mode"] != "router" {
                        continue;
                    }
                    if let Some(other) = rounds.iter().find(|r| {
                        r["mode"] == "v5"
                            && r["size"] == round["size"]
                            && r[side]["exact"]["spent"] == ok["spent"]
                    }) {
                        let direct = raw(&other[side]["exact"]["received"]);
                        let router = raw(&ok["received"]);
                        if index == 0 {
                            assert!(direct >= router);
                        } else {
                            assert_eq!(direct, router);
                        }
                        same_input += 1;
                    }
                }
            }
        }
        assert!(same_input > 0);
    }
}

#[test]
fn surge_wallet_receives_ether_after_tax_on_a_later_block() {
    let d = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/surge_ethereum_wallets.json.gz"
    ));
    let state = &d["state"];
    let liq = raw(&state["liquidity"]);
    let k = raw(&state["liqConst"]);
    let den = raw(&state["DIVISOR"]);
    let stock = raw(&state["stock"]);
    let cases = d["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    assert_eq!(state["fee_exempt"], false);
    for r in cases {
        let paid = raw(&r["input"]);
        let added = paid * raw(&state["buyMul"]) / den;
        let bought = stock - k / (liq + added);
        let gross = liq + added - k / stock;
        let net = gross - gross * (den - raw(&state["sellMul"])) / den;
        assert_eq!(bought, raw(&r["bought"]));
        assert_eq!(gross, raw(&r["gross"]));
        assert_eq!(net, raw(&r["received"]));
        assert_ne!(raw(&r["before"][2]), liq);
        assert_eq!(raw(&r["after_buy"][1]) - raw(&r["before"][1]), bought);
        assert_eq!(raw(&r["after_sell"][0]) - raw(&r["after_buy"][0]), net);
        assert_eq!(raw(&r["after_sell"][3]) - raw(&r["after_buy"][3]), bought);
        assert!(
            r["sell"]["same_block"]["message"]
                .as_str()
                .unwrap()
                .contains("same block")
        );
        for side in ["buy", "sell"] {
            assert!(r[side]["gas"].as_u64().unwrap() > 0);
            assert_eq!(r[side]["minimum_plus_one"]["code"], 3);
            assert_eq!(r[side]["expired"]["code"], 3);
        }
    }
}
