//! Реальные расходы и выплаты каждого поколения, включая два исходящих перевода из одного EVault.
use super::stabull_wallet::{Form,Proof,Terms};
use alloy_primitives::{Address,Bytes,U256};
use flate2::read::GzDecoder;
use serde_json::Value;

fn address(v:&Value)->Address {v.as_str().unwrap().parse().unwrap()}
fn raw(v:&Value)->U256 {U256::from(v.as_u64().unwrap())}

#[test]
fn original_and_protected_orders_recover_the_real_cash() {
    let samples:[(&[u8],&[u8],usize);3]=[
        (include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/eulerswap_ethereum_wallets.json.gz"),
         include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/eulerswap_ethereum_registry.json.gz"),24),
        (include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/eulerswap_base_wallets.json.gz"),
         include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/eulerswap_base_registry.json.gz"),16),
        (include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/eulerswap_ethereum_unlisted_wallets.json.gz"),
         include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/eulerswap_ethereum_directory.json.gz"),16)];
    for (wallet,directory,total) in samples{
        let w:Value=serde_json::from_reader(GzDecoder::new(wallet)).unwrap();
        let d:Value=serde_json::from_reader(GzDecoder::new(directory)).unwrap();
        let mut measured=0;
        for c in w["cases"].as_array().unwrap().iter().filter(|c|c["mode"]!="v5"){
            let v2=c["version"]==2;
            let p=d["generations"][usize::from(v2)]["pools"].as_array().unwrap().iter().find(|p|p["address"]==c["pool"]).unwrap();
            let side=usize::from(p["tokens"][1]==c["output"]);
            let vault=address(&p["config"][side]);
            let vaults=[vault,if v2{address(&p["config"][2+side])}else{vault}];
            let proof=Proof{terms:Terms{form:Form::Euler{v2,guarded:c["mode"]=="module",vaults},pool:address(&c["pool"]),
                quote:Address::ZERO,target:address(&c["spender"]),desired:c["wanted"].as_bool().unwrap().then(||raw(&c["minimum"]))},
                payer:address(&c["payer"]),input:address(&c["input"]),output:address(&c["output"]),maximum:raw(&c["paid"]),
                minimum:raw(&c["minimum"]),deadline:raw(&w["stamp"])+U256::from(240)};
            assert_eq!(proof.data(),c["order"]["data"].as_str().unwrap().parse::<Bytes>().unwrap());
            let logs:Vec<crate::venues::bsc_dex::check::simulate::SimLog>=serde_json::from_value(c["logs"].clone()).unwrap();
            assert_eq!(proof.measured(&logs),Some((raw(&c["paid"]),raw(&c["net"]))));
            measured+=1;
        }
        assert_eq!(measured,total);
        assert!(w["refusals"].as_array().unwrap().iter().all(|x|x["before"]==x["after"] && x["error"]["data"]=="0x34373fbc"));
    }
}

