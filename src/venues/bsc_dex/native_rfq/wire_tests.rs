//! Квитанции двенадцати направленных заявок Native RFQ на обычном кошельке и защищённом модуле.
use super::*;
use flate2::read::GzDecoder;
use std::collections::BTreeSet;

#[test]
fn single_order_receipt_identifies_exact_wallet_payment_and_rejects_mixed_flows() {
    let data = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/native_rfq_ethereum_wallets.json.gz"
    );
    let archive: Value = serde_json::from_reader(GzDecoder::new(data.as_slice())).unwrap();
    let directory: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/native_rfq_ethereum_public.json"
    ))
    .unwrap();
    let pool = addr(&directory["roles"]["roles"][1]["address"]).unwrap();
    let mut quotes = BTreeSet::new();
    let mut directions = BTreeMap::new();
    for r in archive["observed"].as_array().unwrap() {
        assert_eq!(r["status"], "settled");
        let proof = Proof {
            pool,
            payer: addr(&archive["payer"]).unwrap(),
            input: addr(&r["source"]).unwrap(),
            output: addr(&r["target"]).unwrap(),
            sold: quantity(&r["requested"]).unwrap(),
            minimum: quantity(&r["minimum"]).unwrap(),
            maker: addr(&r["maker"]).unwrap(),
            quote_id: FixedBytes::from_slice(&decoded(&r["quoteId"]).unwrap()),
            digest: r["offer_digest"].as_str().unwrap().parse().unwrap(),
        };
        let logs: Vec<Log> = serde_json::from_value(r["logs"].clone()).unwrap();
        let net = quantity(&r["net"]).unwrap();
        assert!(quotes.insert((proof.quote_id, proof.digest)));
        *directions.entry((proof.input, proof.output)).or_insert(0) += 1;
        assert_eq!(
            quantity(&r["before"][0]).unwrap() - quantity(&r["after"][0]).unwrap(),
            proof.sold
        );
        assert_eq!(
            quantity(&r["after"][1]).unwrap() - quantity(&r["before"][1]).unwrap(),
            net
        );
        assert_eq!(proof.received(&logs, None), Some(net));
        assert_eq!(proof.received(&logs, Some(proof.payer)), None);
        for extra in &logs {
            let mut many = logs.clone();
            many.push(extra.clone());
            assert_eq!(proof.received(&many, None), None);
        }
        let mut changed = proof.clone();
        changed.minimum = net + U256::ONE;
        assert_eq!(changed.received(&logs, None), None);
        changed = proof;
        changed.quote_id = FixedBytes::ZERO;
        assert_eq!(changed.received(&logs, None), None);
    }
    assert_eq!(quotes.len(), 12);
    assert_eq!(directions.len(), 6);
    assert!(directions.values().all(|n| *n == 2));
}
use std::collections::BTreeMap;

#[test]
fn protected_receipt_needs_saved_digest_maker_and_wallet_credits() {
    let data = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/native_rfq_ethereum_protected.json.gz"
    );
    let archive: Value = serde_json::from_reader(GzDecoder::new(data.as_slice())).unwrap();
    let directory: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/native_rfq_ethereum_public.json"
    ))
    .unwrap();
    let pool = addr(&directory["roles"]["roles"][1]["address"]).unwrap();
    let payer = addr(&archive["payer"]).unwrap();
    let rows = archive["observed"].as_array().unwrap();
    assert_eq!(rows.len(), 12);
    for r in rows {
        let p = Proof {
            pool,
            payer,
            input: addr(&r["source"]).unwrap(),
            output: addr(&r["target"]).unwrap(),
            sold: quantity(&r["requested"]).unwrap(),
            minimum: quantity(&r["minimum"]).unwrap(),
            maker: addr(&r["maker"]).unwrap(),
            quote_id: FixedBytes::from_slice(&decoded(&r["quoteId"]).unwrap()),
            digest: r["offer_digest"].as_str().unwrap().parse().unwrap(),
        };
        let module = addr(&r["module"]).unwrap();
        let logs: Vec<Log> = serde_json::from_value(r["logs"].clone()).unwrap();
        let received = quantity(&r["net"]).unwrap();
        assert_eq!(p.received(&logs, Some(module)), Some(received));
        assert_eq!(
            quantity(&r["before"][0]).unwrap() - quantity(&r["after"][0]).unwrap(),
            p.sold
        );
        assert_eq!(
            quantity(&r["after"][1]).unwrap() - quantity(&r["before"][1]).unwrap(),
            received
        );
        let mut wrong = p.clone();
        wrong.digest = B256::ZERO;
        assert!(wrong.received(&logs, Some(module)).is_none());
        let mut wrong = p.clone();
        wrong.maker = Address::ZERO;
        assert!(wrong.received(&logs, Some(module)).is_none());
        let missing = logs
            .iter()
            .filter(|l| l.address() != module)
            .cloned()
            .collect::<Vec<_>>();
        assert!(p.received(&missing, Some(module)).is_none());
        assert!(p.received(&logs, Some(Address::ZERO)).is_none());
    }
}
