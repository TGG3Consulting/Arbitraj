//! Проверка подписанного поручения по двум действительным кошельковым архивам на Base.
use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn only_the_signed_wallets_payment_survives_restore_with_and_without_tax_swap() {
    let snapshot =
        |bytes: &[u8]| -> Value { serde_json::from_reader(GzDecoder::new(bytes)).unwrap() };
    let directory = snapshot(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/basestonk_base_directory.json.gz"
    ));
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let mut total = 0;
    let mut nested = 0;
    for raw in [
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/basestonk_base_protected_primary.json.gz"
        )[..],
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/basestonk_base_protected_secondary.json.gz"
        )[..],
    ] {
        let d = snapshot(raw);
        let origin = &d["protected"];
        for r in d["results"].as_array().unwrap() {
            let k = &directory["markets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["id"] == r["id"])
                .unwrap()["key"];
            let amount = r["paid"].to_string().parse::<U256>().unwrap();
            let received = r["received"].to_string().parse::<U256>().unwrap();
            let p = Proof {
                terms: Terms {
                    chain: 8453,
                    module: addr(&origin["module_simulated"]),
                    router: addr(&origin["router"]),
                    manager: addr(&origin["manager"]),
                    hook: addr(&r["hook"]),
                    tokens: [addr(&k[0]), addr(&k[1])],
                    fee: U24::from(k[2].as_u64().unwrap() as u32),
                    spacing: I24::try_from(k[3].as_i64().unwrap()).unwrap(),
                },
                payer: addr(&d["payer"]),
                input: addr(&r["src"]),
                output: addr(&r["dst"]),
                amount,
                minimum: received,
                deadline: U256::from(d["timestamp"].as_u64().unwrap() + 90),
            };
            let logs: Vec<Log> = serde_json::from_value(r["logs"].clone()).unwrap();
            let saved: Proof = serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
            assert_eq!(saved.data(), p.data());
            assert_eq!(
                saved.amounts(&logs, p.payer, p.input, p.output),
                Some((amount, received))
            );
            assert!(
                p.amounts(&logs, p.terms.module, p.input, p.output)
                    .is_none()
            );
            let mut strict = p.clone();
            strict.minimum += U256::ONE;
            assert!(strict.amounts(&logs, p.payer, p.input, p.output).is_none());
            assert_eq!(r["strict"]["success"], false);
            assert_eq!(r["expired"]["code"], 3);
            total += 1;
            nested += usize::from(r["pool_swaps"] == 2);
        }
    }
    assert_eq!((total, nested), (40, 10));
}
