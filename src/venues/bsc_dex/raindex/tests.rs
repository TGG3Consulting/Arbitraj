//! Прямое сопоставление принятой схемы ABI с выплатами своего архивного кошелька на каждой сети.
use super::*;
use alloy::rpc::types::Log;
use alloy::sol_types::{SolCall, SolValue};
use flate2::read::GzDecoder;
use serde_json::{Value, from_value};
#[test]
fn original_order_and_one_payment_match_own_wallet_on_both_networks() {
    for (archive,count) in [
        (include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/raindex_base_protected_wallets.json.gz").as_slice(),42),
        (include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/raindex_ethereum_protected_wallets.json.gz").as_slice(),3)
    ]{
        let j:Value=serde_json::from_reader(GzDecoder::new(archive)).unwrap();
        let owner=from_value(j["payer"].clone()).unwrap();let mut filled=0;
        for a in j["pairs"].as_array().unwrap().iter().flat_map(|p|p["cases"].as_array().unwrap()){
            for v in a.as_object().unwrap().values().filter(|v|v["stage"]=="paid"){
                let gas=v["gas"].as_u64().unwrap_or_else(||v["gas"].as_str().unwrap().parse().unwrap());assert!(gas>0);
                let module=from_value(v["target"].clone()).unwrap();
                let book=from_value(v["book"].clone()).unwrap();
                let p=Profile{module,book,chain:1,version:v["version"].as_u64().unwrap() as u8,book_hash:B256::ZERO,module_hash:B256::ZERO};
                let blob:Bytes=from_value(v["action"].clone()).unwrap();
                let encoded=if p.version==4{wire::takeOrders2Call::abi_decode_validate(&blob).unwrap().config.orders[0].order.abi_encode()}
                     else{wire::takeOrders4Call::abi_decode_validate(&blob).unwrap().config.orders[0].order.abi_encode()};
                let hash=from_value(v["order"].clone()).unwrap();let order=Arc::new(wire::Order::decode(p.version,&encoded,hash).unwrap());
                let i=v["i"].as_u64().unwrap() as usize;let z=v["j"].as_u64().unwrap() as usize;
                let l=Leg{p,hash,i,j:z,order,input:from_value(v["pay"].clone()).unwrap(),output:from_value(v["receive"].clone()).unwrap()};
                let spent:U256=v["paid"].as_str().unwrap().parse().unwrap();let net:U256=v["net"].as_str().unwrap().parse().unwrap();
                let payment=payment::Payment::new(&l,owner,blob.clone(),0,(spent,spent,net));
                let mut logs:Vec<Log>=from_value(v["logs"].clone()).unwrap();
                assert_eq!(payment.saved.amounts(&logs),Some((spent,net)));
                let repeated=logs.iter().find(|e|e.address()==book&&e.topics().first()==Some(&if p.version==4{<wire::TakeOrderV2 as alloy::sol_types::SolEvent>::SIGNATURE_HASH}else{<wire::TakeOrderV3 as alloy::sol_types::SolEvent>::SIGNATURE_HASH})).unwrap().clone();
                logs.push(repeated);assert!(payment.saved.amounts(&logs).is_none());filled+=1;
            }
        }
        assert_eq!(filled,count);
    }
}
