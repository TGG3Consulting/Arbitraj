use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;

fn integer(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

fn from_row(d: &Value, row: &Value) -> Market {
    let tokens = [addr(&row["tokens"][0]), addr(&row["tokens"][1])];
    let oracle = &d["oracles"][row["oracle"].as_str().unwrap()];
    Market {
        tokens,
        atf: addr(&d["fee_token"]),
        tier: row["fee"].as_u64().unwrap() as u16,
        reserves: std::array::from_fn(|i| integer(&row["reserves"][i])),
        balances: std::array::from_fn(|i| integer(&row["balances"][i])),
        dividends: integer(&row["escrow"]),
        lp_supply: integer(&row["supply"]),
        oracle_address: (!oracle.is_null()).then(|| addr(&row["oracle"])),
        oracle: (!oracle.is_null()).then(|| {
            (
                tokens
                    .iter()
                    .position(|t| *t == addr(&oracle["asset"]))
                    .unwrap(),
                integer(&oracle["rate_q112"]),
            )
        }),
    }
}

#[test]
fn fully_funded_fill_prices_refilling_the_fee_token_in_the_input_asset() {
    let registry = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/antfarm_ethereum_registry.json.gz"
    );
    let protected = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/antfarm_protected_ethereum.json.gz"
    );
    let d: Value = serde_json::from_reader(GzDecoder::new(registry.as_slice())).unwrap();
    let controls: Value = serde_json::from_reader(GzDecoder::new(protected.as_slice())).unwrap();
    let rows = d["pairs"].as_array().unwrap();
    let weth = alloy_primitives::address!("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2");
    let fee_market = from_row(
        &d,
        rows.iter()
            .find(|r| {
                addr(&r["tokens"][0]) == addr(&d["fee_token"])
                    && addr(&r["tokens"][1]) == weth
                    && r["fee"] == 100
            })
            .unwrap(),
    );
    let cases = controls["cases"].as_array().unwrap();
    for trial in cases {
        let pool = from_row(
            &d,
            rows.iter().find(|r| r["pair"] == trial["pool"]).unwrap(),
        );
        let f = pool
            .settle(weth, integer(&trial["primary_input"]), false)
            .unwrap();
        let restored = fee_market.acquire_fee(weth, f.separate_atf).unwrap();
        if trial["mode"] == "buy_fee" {
            use crate::core::types::AntfarmBook;
            use crate::venues::bsc_dex::pools::abi_v2v3::IV2Pair;
            use alloy::sol_types::SolEvent;
            let b = AntfarmBook {
                limits: None,
                proceeds: None,
                market: pool.clone(),
                funding: vec![(fee_market.clone(), None)],
            };
            let budget = integer(&trial["net"][0]);
            let (fill, post) = b.after(weth, budget).expect("funded protected buy");
            assert_eq!(
                (fill.amount_in, fill.amount_out),
                (budget, integer(&trial["net"][1]))
            );
            let logs: Vec<alloy::rpc::types::Log> =
                serde_json::from_value(trial["logs"].clone()).unwrap();
            for (addr, m) in [
                (addr(&trial["pool"]), &post.market),
                (
                    addr(
                        &rows
                            .iter()
                            .find(|r| {
                                r["fee"] == 100
                                    && addr(&r["tokens"][0]) == pool.atf
                                    && addr(&r["tokens"][1]) == weth
                            })
                            .unwrap()["pair"],
                    ),
                    &post.funding[0].0,
                ),
            ] {
                let l = logs
                    .iter()
                    .find(|l| {
                        l.address() == addr
                            && l.topics().first() == Some(&IV2Pair::Sync::SIGNATURE_HASH)
                    })
                    .unwrap();
                let sync = IV2Pair::Sync::decode_log_data(l.data()).unwrap();
                assert_eq!(
                    m.reserves,
                    [U256::from(sync.reserve0), U256::from(sync.reserve1)]
                );
            }
        }
        assert_eq!(restored.output, f.separate_atf);
        assert_eq!(restored.debit, integer(&trial["funding_cost"]));
        assert_eq!(f.output, integer(&trial["net"][1]));
        let held = trial["mode"] == "held_fee";
        assert_eq!(
            f.debit + if held { U256::ZERO } else { restored.debit },
            integer(&trial["net"][0])
        );
        assert_eq!(
            if held { f.separate_atf } else { U256::ZERO },
            integer(&trial["net"][2])
        );
    }
    assert_eq!(cases.len(), 16);
}

#[test]
fn both_router_methods_match_owner_receipts_in_both_directions() {
    let fixture = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/antfarm_wallets_ethereum.json.gz"
    );
    let data: Value = serde_json::from_reader(GzDecoder::new(fixture.as_slice())).unwrap();
    let d = &data["directory"];
    let mut yes = 0;
    let mut no = 0;
    for trace in data["outcomes"].as_array().unwrap() {
        let row = d["pairs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["pair"] == trace["pool"])
            .unwrap();
        let market = from_row(d, row);
        let tokens = market.tokens;
        for trial in trace["examples"].as_array().unwrap() {
            let mut state = market.clone();
            for leg in [trial.get("forward"), trial.get("reverse")]
                .into_iter()
                .flatten()
            {
                if leg.get("source").is_none() {
                    assert_eq!(leg["rejected"], "input below router rounding");
                    continue;
                }
                let source = addr(&leg["source"]);
                let supporting = leg["supporting"].as_bool().unwrap();
                let fill = state.settle(source, integer(&leg["budget"]), supporting);
                if leg.get("output").is_none() {
                    assert!(fill.is_none(), "{leg}");
                    no += 1;
                } else {
                    let f = fill
                        .unwrap_or_else(|| panic!("missed {} {}", row["pair"], leg["transaction"]));
                    assert_eq!(f.output, integer(&leg["output"]));
                    assert_eq!(f.parameter, integer(&leg["parameter"]));
                    assert_eq!(f.fee_limit, integer(&leg["fee_ceiling"]));
                    let delta = |token: Address| -> U256 {
                        leg["measured"][format!("{token:#x}")]
                            .as_str()
                            .unwrap()
                            .trim_start_matches('-')
                            .parse()
                            .unwrap()
                    };
                    assert_eq!(f.debit, delta(source));
                    if !tokens.contains(&state.atf) {
                        assert_eq!(f.separate_atf, delta(state.atf));
                    }
                    state = f.next;
                    yes += 1;
                }
            }
        }
    }
    assert_eq!((yes, no), (98, 24));
}

#[test]
fn paying_fees_from_proceeds_matches_eight_atomic_wallet_fills() {
    use crate::core::types::AntfarmBook;
    use crate::venues::bsc_dex::math::antfarm_output::Funds;
    let reg = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/antfarm_ethereum_registry.json.gz"
    );
    let paid = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/antfarm_output_ethereum.json.gz"
    );
    let d: Value = serde_json::from_reader(GzDecoder::new(reg.as_slice())).unwrap();
    let result: Value = serde_json::from_reader(GzDecoder::new(paid.as_slice())).unwrap();
    let markets = d["pairs"].as_array().unwrap();
    let mut count = 0;
    for r in result["cases"].as_array().unwrap() {
        let primary = from_row(&d, markets.iter().find(|m| m["pair"] == r["pair"]).unwrap());
        let event = r["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v["logs"].as_array())
            .flatten()
            .find(|l| l["address"] == result["module"])
            .unwrap();
        let token =
            |i: usize| Address::from_word(event["topics"][i].as_str().unwrap().parse().unwrap());
        let (source, output) = (token(2), token(3));
        let tier = r["tier"].as_u64().unwrap() as u16;
        let fee_asset = primary.atf;
        let fund = from_row(
            &d,
            markets
                .iter()
                .find(|m| {
                    m["fee"] == tier
                        && addr(&m["tokens"][0]) == fee_asset
                        && addr(&m["tokens"][1]) == output
                })
                .unwrap(),
        );
        let book = AntfarmBook {
            limits: None,
            proceeds: None,
            market: primary,
            funding: vec![(fund, None)],
        };
        let q = integer(&r["qty"]);
        let cost = integer(&r["cost"]);
        let fee = integer(&r["atf"]);
        let loan = r["loan"].as_bool().unwrap();
        let balances = if loan {
            Funds::Vault {
                output_balance: cost,
                fee_per_1e18: integer(&result["fee_percentage"]),
            }
        } else {
            Funds::Wallet { atf: fee }
        };
        let fill = book.paid_from_output(source, q, tier, balances).unwrap();
        assert_eq!(
            (
                fill.primary.debit,
                fill.primary.output,
                fill.refill.debit,
                fill.refill.output
            ),
            (q, integer(&r["gross"]), cost, fee)
        );
        assert_eq!(
            (fill.credit, fill.loan_fee),
            (integer(&r["credit"]), U256::ZERO)
        );
        let mut alternate = book.clone();
        alternate.proceeds = Some((tier, balances));
        let inverse = alternate
            .output_order(source, false, fill.credit)
            .expect("net payout has an executable archived witness");
        assert!(inverse.credit >= fill.credit && inverse.primary.debit <= q);
        if inverse.primary.debit > U256::ONE {
            assert!(
                alternate
                    .paid_from_output(source, inverse.primary.debit - U256::ONE, tier, balances)
                    .is_none_or(|smaller| smaller.credit < fill.credit)
            );
        }
        let mut all = book.clone();
        let mut loans = [None, None];
        loans[usize::from(all.market.tokens[1] == output)] = loan.then_some(balances);
        all.limits = Some(crate::core::types::AntfarmLimits {
            payer: Address::ZERO,
            module: Address::ZERO,
            stock: (!loan).then_some(balances),
            loans,
        });
        let visible = all
            .swap(source, true, q)
            .expect("model must expose routes funded from the payout before execution search");
        assert_eq!((visible.amount_in, visible.amount_out), (q, fill.credit));
        assert!(
            book.paid_from_output(
                source,
                q,
                tier,
                Funds::Vault {
                    output_balance: cost - U256::ONE,
                    fee_per_1e18: U256::ZERO
                }
            )
            .is_none()
        );
        assert!(
            book.paid_from_output(
                source,
                q,
                tier,
                Funds::Wallet {
                    atf: fee - U256::ONE
                }
            )
            .is_none()
        );
        let interest = book
            .paid_from_output(
                source,
                q,
                tier,
                Funds::Vault {
                    output_balance: cost,
                    fee_per_1e18: U256::ONE,
                },
            )
            .unwrap();
        assert_eq!(interest.credit + U256::ONE, fill.credit);
        count += 1;
    }
    assert_eq!(count, 8);
}
