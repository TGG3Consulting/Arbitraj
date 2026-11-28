//! Оценка до firm — медиана записанных одиночных PMM при отправке собственным EOA, умноженная на цену текущей головы.
use crate::venues::bsc_dex::{num,parts::Parts,quote::gas_base,send};
use alloy_primitives::{Address,U256};
use rust_decimal::Decimal;

pub(super) fn sampled(input:&str,output:&str)->u64 {
    match (input,output) {
        ("wrapped-native","tether-net")=>269149,("tether-net","wrapped-native")=>291997,("fiat-usdc","wrapped-native")=>300495,
        ("tether-net","fiat-usdc")=>303653,("fiat-usdc","tether-net")=>308495,("wrapped-native","fiat-usdc")=>272853,
        ("tether-net","exact-uni")=>305072,
        // В 19 исполнениях relayer иной отправитель; общий ориентир остальных 82 прямых платежей.
        _=>286261,
    }
}
pub(super) fn indicative(p:&Parts,head:u64,input:Address,output:Address)->Option<Decimal>{
    let assets=super::roles::assets(&p.ctx)?;
    let i=assets.iter().find(|a|a.token.address==input)?;
    let o=assets.iter().find(|a|a.token.address==output)?;
    let (block,base)=p.gas.known()?;
    if head!=block{return None;}
    let fees=gas_base::fees(&p.ctx,base).ok()?;
    let floor=send::planned_floor(&p.ctx).ok()?;
    let fees=send::tx_price::floored(fees,floor,base)?;
    let unit=base.checked_add(U256::from(fees.tip))?;
    num::to_decimal(unit.checked_mul(U256::from(sampled(&i.id,&o.id)))?,18)
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::venues::bsc_dex::tokenlon::wire::abi::FillOrder;
    use alloy::{rpc::types::Log,sol_types::SolEvent};
    use flate2::read::GzDecoder;
    use serde_json::Value;
    use std::collections::BTreeMap;
    #[test]
    fn archive_indicative_gas_medians_only_include_direct_payers(){
        let data=include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_expired_pmm.json.gz");
        let archive:Value=serde_json::from_reader(GzDecoder::new(&data[..])).unwrap();
        let mut groups=BTreeMap::new();
        let mut samples=Vec::new();
        for row in archive["rows"].as_array().unwrap(){
            let sender:Address=row["tx"]["from"].as_str().unwrap().parse().unwrap();
            let payment=row["logs"].as_array().unwrap().iter().find_map(|value|{
                let log=Log{inner:serde_json::from_value(value.clone()).unwrap(),..Default::default()};
                (log.address()==alloy_primitives::address!("8d90113a1e286a5ab3e496fbd1853f265e5913c6") && log.topics().first()==Some(&FillOrder::SIGNATURE_HASH))
                    .then(||FillOrder::decode_log_data_validate(&log.inner.data).ok()).flatten()
            }).unwrap();
            if sender==payment.userAddr{
                let gas=row["gasUsed"].as_u64().unwrap();
                samples.push(gas);
                groups.entry((payment.takerAssetAddr,payment.makerAssetAddr)).or_insert_with(Vec::new).push(gas);
            }
        }
        assert_eq!(samples.len(),82);
        let median=|list:&mut Vec<u64>|{list.sort_unstable();list[(list.len()-1)/2]};
        assert_eq!(sampled("unknown","unknown"),median(&mut samples));
        assert_eq!(groups.len(),7);
        let evidence:Value=serde_json::from_str(include_str!("../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_roles.json")).unwrap();
        let rows:Vec<String>=serde_json::from_value(evidence["assets"].clone()).unwrap();
        let tokens=super::super::roles::asset_rows(evidence["chain"].as_u64().unwrap(), &rows).unwrap();
        let id=|address:Address|tokens.iter().find(|row|row.token.address==address).unwrap().id.as_str();
        for ((i,o),mut obs) in groups {assert_eq!(sampled(id(i),id(o)),median(&mut obs));}
    }
}
