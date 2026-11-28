//! Квитанция прямого Stelaxis: собственный вход через модуль, реальная касса и конечный получатель.
use super::Proof;
use crate::venues::bsc_dex::exec::{fill, prop_abi::IWrappedProp};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address,U256};
pub(super) fn outcome(q:&Proof,logs:&[Log])->Option<(Address,U256,U256)> {
    let t=q.terms;
    if t.target==t.venue || t.fallback_allowed || t.module_hash.is_none() || t.via.is_some() { return None; }
    let mut swaps=logs.iter().filter(|l|l.address()==t.venue && l.topic0()==Some(&IWrappedProp::Swapped::SIGNATURE_HASH));
    let s=IWrappedProp::Swapped::decode_log_data(&swaps.next()?.inner.data).ok()?;
    if swaps.next().is_some() || (s.sender,s.recipient,s.tokenIn,s.tokenOut,s.amountIn)!=(t.target,t.target,q.input,q.output,q.amount)
        || s.amountIn.is_zero() || s.amountOut.is_zero(){return None;}
    let moved=|token,from,to|fill::moved_logs(logs,token,from,to);
    let debit=moved(q.input,Some(q.payer),None).checked_sub(moved(q.input,None,Some(q.payer)))?;
    let credit=moved(q.output,None,Some(q.payer)).checked_sub(moved(q.output,Some(q.payer),None))?;
    if debit!=q.amount || credit.is_zero() || credit<q.minimum || credit>s.amountOut ||
        moved(q.input,Some(q.payer),Some(t.target))!=debit || moved(q.input,Some(t.target),Some(t.venue))!=debit ||
        moved(q.input,Some(t.venue),Some(t.vault))!=debit || moved(q.output,Some(t.vault),Some(t.target))!=s.amountOut ||
        moved(q.output,Some(t.target),Some(q.payer))!=credit {return None;}
    Some((t.venue,debit,credit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::bsc_dex::{check::simulate::SimLog,exec::{carbon_binding::Code,prop_wallet::Terms}};
    use flate2::read::GzDecoder;
    use serde_json::Value;
    use alloy_primitives::B256;
    fn read(bytes:&[u8])->Value {serde_json::from_reader(GzDecoder::new(bytes)).unwrap()}
    fn addr(v:&Value)->Address{v.as_str().unwrap().parse().unwrap()}
    fn qty(v:&Value)->U256{U256::from_str_radix(v.as_str().unwrap(),10).unwrap()}
    #[test]
    fn original_and_single_use_stelaxis_pay_the_same_owner_in_both_directions() {
        let direct=read(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/stelaxis_ethereum_own.json.gz"));
        let slot=read(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/stelaxis_ethereum_slot.json.gz"));
        let card:Value=serde_json::from_str(include_str!("../../../../ui/src/assets/stelaxis-slot-ethereum.json")).unwrap();
        let venue=addr(&slot["venue"]); let roles=&slot["roles"];
        let hash=|v:&Value|v.as_str().unwrap().parse::<B256>().unwrap();
        let code=|v:&Value|Code{shell:hash(&v["hash"]),implementation:addr(&v["implementation"]),runtime:hash(&v["implementationHash"])};
        let terms=Terms{target:addr(&slot["module"]),router:venue,venue,vault:addr(&roles["vault"]["address"]),
            fallback:addr(&roles["registry"]["address"]),code:code(&roles["market"]),venue_code:Some(code(&roles["market"])),
            vault_code:Some(code(&roles["vault"])),fallback_allowed:false,via:None,module_hash:Some(hash(&card["markets"][format!("{venue:#x}")]["runtimeKeccak"]))};
        let original=direct["rows"].as_array().unwrap();
        let protected=slot["rows"].as_array().unwrap();assert_eq!((original.len(),protected.len()),(20,20));
        assert_eq!(direct["origin"],slot["origin"]);
        let mut payments=0;
        for (a,b) in original.iter().zip(protected) {
            for key in ["input","output","amount"] {assert_eq!(a[key],b[key]);}
            let result=&b["withUpdate"];assert_eq!(result["status"],a["withUpdate"]["status"]);
            if result["status"]!="paid" {assert_eq!(result["before"],result["after"]);continue;}
            let q=Proof{terms,payer:addr(&slot["owner"]),input:addr(&b["input"]),output:addr(&b["output"]),
                amount:qty(&b["amount"]),minimum:qty(&result["minimum"]),deadline:U256::from(result["deadline"].as_u64().unwrap())};
            let logs:Vec<SimLog>=serde_json::from_value(result["logs"].clone()).unwrap();
            let spent=qty(&result["before"][0])-qty(&result["after"][0]);let received=qty(&result["after"][1])-qty(&result["before"][1]);
            assert_eq!(q.measured(&logs),Some((spent,received)));
            assert_eq!(a["withUpdate"]["credit"],result["credit"]);assert_eq!(a["withUpdate"]["debit"],result["debit"]);
            for name in ["withoutUpdate","expired","tight"] {assert_eq!(b[name]["status"],"reverted");assert_eq!(b[name]["before"],b[name]["after"]);}
            let mut wrong=q;wrong.minimum=received+U256::ONE;assert!(wrong.measured(&logs).is_none());
            payments+=1;
        }
        assert_eq!(payments,4);
    }
}
