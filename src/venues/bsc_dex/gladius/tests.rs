//! Истёкшие подписи принадлежат двум тестовым кошелькам из воспроизведения в GitHub Actions.
use super::{
    abi::*,
    model::{ApiOrder, Offer},
    profile::{Profile, Role},
    simulate::Payment,
    *,
};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use flate2::read::GzDecoder;
use serde_json::{Value, from_value};
use std::collections::BTreeSet;

#[test]
fn archived_owners_full_and_partial_signed_orders_match_both_facts_and_terminal_balances() {
    for archive in [
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/gladius_base_protected.json.gz"
        )
        .as_slice(),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/gladius_ethereum_protected.json.gz"
        )
        .as_slice(),
    ] {
        let j: Value = serde_json::from_reader(GzDecoder::new(archive)).unwrap();
        let role = |address: &Value, hash: &Value| Role {
            address: from_value(address.clone()).unwrap(),
            hash: from_value(hash.clone()).unwrap(),
        };
        let rr = &j["roles"]["reactor"];
        let qq = &j["roles"]["quoter"];
        let p = Profile {
            chain: j["chain"].as_u64().unwrap(),
            reactor: role(&rr["address"], &rr["runtime"]),
            implementation: role(&rr["implementation"], &rr["implementation_runtime"]),
            quoter: role(&qq["address"], &qq["runtime"]),
            module: role(&j["module"], &j["runtime"]),
        };
        let mut seen = BTreeSet::new();
        for row in j["cases"].as_array().unwrap() {
            let raw: ApiOrder = from_value(row["api_fixture"].clone()).unwrap();
            let deadline = row["deadline"].as_u64().unwrap();
            let mut foreign = raw.clone();
            foreign.chain_id = p.chain + 1;
            assert!(Offer::parse(foreign, &p, deadline - 1).is_none());
            let mut closed = raw.clone();
            closed.order_status = "filled".into();
            assert!(Offer::parse(closed, &p, deadline - 1).is_none());
            assert!(Offer::parse(raw.clone(), &p, deadline).is_none());
            let offer = Offer::parse(raw, &p, deadline - 1).unwrap();
            let owner = from_value(row["owner"].clone()).unwrap();
            let amount = |k: &str| row[k].as_str().unwrap().parse::<U256>().unwrap();
            let (q, d, cap) = (amount("quantity"), amount("spent"), amount("cap"));
            let partial = row["partial"].as_bool().unwrap();
            let payment = Payment::new(
                p,
                owner,
                &offer,
                q,
                if partial { d } else { U256::ZERO },
                cap,
                q,
            );
            let digest: B256 = from_value(row["input_digest"].clone()).unwrap();
            assert_eq!(payment.saved.call.calldata, digest);
            let logs: Vec<Log> = from_value(row["logs"].clone()).unwrap();
            assert_eq!(payment.saved.amounts(&logs), Some((d, q)));
            for topic in [
                IWallet::Settled::SIGNATURE_HASH,
                IReactor::Fill::SIGNATURE_HASH,
            ] {
                let mut uniq = logs.clone();
                let index = uniq
                    .iter()
                    .position(|l| l.topics().first() == Some(&topic))
                    .unwrap();
                uniq.push(uniq[index].clone());
                assert_eq!(payment.saved.amounts(&uniq), None);
                let mut missing = logs.clone();
                missing.remove(index);
                assert_eq!(payment.saved.amounts(&missing), None);
            }
            assert_eq!(
                Payment::new(p, Address::ZERO, &offer, q, U256::ZERO, cap, q)
                    .saved
                    .amounts(&logs),
                None
            );
            assert_eq!(
                Payment::new(p, owner, &offer, q, U256::ZERO, d - U256::ONE, q)
                    .saved
                    .amounts(&logs),
                None
            );
            assert_eq!(
                Payment::new(p, owner, &offer, q, U256::ZERO, cap, q + U256::ONE)
                    .saved
                    .amounts(&logs),
                None
            );
            assert!(seen.insert((offer.pair(), partial)));
        }
        assert_eq!(seen.len(), 4);
    }
}
