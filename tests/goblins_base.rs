//! Реестр и оплаченные направления Goblins. Проверяем настройки и исходное финансирование DEGEN.
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn archive(bytes: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(bytes)).unwrap()
}

#[test]
fn directory_and_wallets_use_the_first_run_factory_on_base() {
    let dir = archive(include_bytes!("data/first_run_dex_2026-11-26/goblins_base_directory.json.gz"));
    let eth = archive(include_bytes!("data/first_run_dex_2026-11-26/goblins_base_wallets.json.gz"));
    let cash = archive(include_bytes!("data/first_run_dex_2026-11-26/goblins_base_cash_wallets.json.gz"));
    let profiles: Vec<Value> = serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let p = profiles.iter().find(|p| p["id"] == "goblins-base").unwrap();
    let s = &p["settings"];
    assert_eq!(p["chain_id"], 8453);
    assert_eq!(s["dex.kind"], "registered_uni_v3");
    assert_eq!(s["dex.v3_wallet_router_abi"], "router02_fee");
    assert_eq!(dir["factory"], s["dex.factory"]);
    let tiers: BTreeSet<_> = s["dex.v3_fee_tiers"].as_array().unwrap().iter().map(|k|k.as_str().unwrap()).collect();
    assert_eq!(tiers,dir["tiers"].as_object().unwrap().keys().map(String::as_str).collect());
    let pools=dir["pools"].as_array().unwrap();
    assert_eq!(dir["nonce"].as_u64(), Some(1+pools.len() as u64));
    assert_eq!(pools.len(),45);
    let exact: BTreeSet<_>=s["dex.router_exact_tokens"].as_array().unwrap().iter().map(|v|v.as_str().unwrap().split_once(' ').unwrap().0).collect();
    let mut matched = BTreeSet::new();
    for (paid, expected, admitted) in [(&eth,12,false),(&cash,8,true)] {
        assert_eq!(paid["selector"],dir["selector"]);
        assert_eq!(paid["state_root"],dir["state_root"]);
        assert_eq!(paid["module_hash"],s["dex.module_runtime_hashes"][0]);
        assert_eq!(paid["init"],s["dex.pool_code_hashes"][0]);
        assert!(paid["unavailable"].as_array().unwrap().is_empty());
        for (role,k) in [("factory","dex.factory"),("router","dex.router"),("quoter","dex.quoter"),("lens","dex.tick_lens")] {
            assert_eq!(paid["roles"][role]["address"],s[k]);
        }
        let mut grouped = BTreeMap::new();
        for c in paid["cases"].as_array().unwrap() {
            let text=|k:&str| c[k].as_str().unwrap();
            let wire=text("wire");
            let pool=text("pool");
            let input=text("input");
            let output=text("output");
            let spent=text("spent").parse::<u128>().unwrap();
            let credit=text("received").parse::<u128>().unwrap();
            assert!(spent>0 && credit>0);
            let market=pools.iter().find(|m|m["pool"]==pool).unwrap();
            let tokens=market["tokens"].as_array().unwrap();
            assert!(tokens.contains(&c["input"]) && tokens.contains(&c["output"]) && input!=output);
            assert_eq!(exact.contains(input) && exact.contains(output),admitted);
            assert_eq!(c["refusals"].as_array().unwrap().len(),2);
            assert_eq!(c["logs"].as_array().unwrap().iter().filter(|l| l["address"].as_str().is_some_and(|a|a.eq_ignore_ascii_case(pool))
                && l["topics"][0]=="0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67").count(),1);
            assert!(c["approval_gas"].as_u64().unwrap()>0 && c["gas"].as_u64().unwrap()>0);
            matched.insert(pool);
            let forms=grouped.entry((pool,input,spent)).or_insert_with(BTreeMap::new);
            assert!(forms.insert(wire,credit).is_none());
        }
        assert_eq!(grouped.len(),expected);
        for forms in grouped.values() {
            assert_eq!(forms.len(),3);
            assert_eq!(forms["module"],forms["router02"]);
            assert_eq!(forms["module"],forms["v5"]);
        }
    }
    assert_eq!(matched.len(),5);
    for f in cash["funding"].as_array().unwrap() {
        assert_eq!(f["pools"].as_array().unwrap().len(),2);
        assert!(f["spent"].as_str().unwrap().parse::<u128>().unwrap()>0);
        let got = f["received"].as_str().unwrap();
        assert!(cash["cases"].as_array().unwrap().iter().any(|c| c["input"]=="0x4ed4e862860bed51a9570b96d89af5e1b0efefed" && c["spent"]==got));
    }
}
