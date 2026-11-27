//! Проверяет совпадение подписываемых байтов и восстановления трёх кошельковых форм по независимым квитанциям.
use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;

fn word(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn restore_cash_and_signed_request_for_exact_curve_router_and_module_orders() {
    let a: &[&[u8]] = &[
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/stabull_base_native_wallets.json.gz"
        )
        .as_slice(),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/stabull_base_protected_wallets.json.gz"
        )
        .as_slice(),
    ];
    let mut seen = [0; 3];
    for &raw in a {
        let d: Value = serde_json::from_reader(GzDecoder::new(raw)).unwrap();
        for row in d["records"].as_array().unwrap() {
            for case in row["cases"].as_array().unwrap() {
                let (idx, form) = match case["mode"].as_str().unwrap() {
                    "pool" => (0, Form::Curve),
                    "router" => (1, Form::Router),
                    "module" => (2, Form::Module),
                    _ => continue,
                };
                for leg in ["buy", "sell", "buy_target", "sell_target"] {
                    let r = &case[leg];
                    if r["stage"] != "paid" {
                        continue;
                    }
                    let payer = addr(&d["payer"]);
                    let input = addr(&r["input"]);
                    let output = addr(&r["output"]);
                    let paid = word(&r["paid"]);
                    let received = word(&r["net"]);
                    let specified = word(&r["specified"]);
                    let terms = Terms {
                        form,
                        pool: addr(&row["pool"]),
                        quote: if form == Form::Router {
                            addr(&case["buy"]["input"])
                        } else {
                            Address::ZERO
                        },
                        target: addr(&r["calldata"]["to"]),
                        desired: (r["exact_output"] == true).then_some(specified),
                    };
                    let data: Bytes = r["calldata"]["data"].as_str().unwrap().parse().unwrap();
                    let deadline = U256::from_be_slice(&data[data.len() - 32..]);
                    let mut p = Proof {
                        terms,
                        payer,
                        input,
                        output,
                        maximum: paid,
                        minimum: if terms.desired.is_some() {
                            specified
                        } else {
                            received
                        },
                        deadline,
                    };
                    assert_eq!(p.data(), data);
                    let logs: Vec<Log> = serde_json::from_value(r["logs"].clone()).unwrap();
                    assert_eq!(
                        p.amounts(&logs, payer, input, output),
                        Some((paid, received))
                    );
                    assert!(p.amounts(&logs, Address::ZERO, input, output).is_none());
                    p.terms.pool = Address::ZERO;
                    assert!(p.amounts(&logs, payer, input, output).is_none());
                    seen[idx] += 1;
                }
            }
        }
    }
    assert_eq!(seen, [40, 20, 40]);
}
