//! Native Base. Подписанные заявки своего адреса, его фактические платежи и штатный выбор физической валюты.
//! Источники — публичная книга Native, профиль первого запуска и Actions #46096868177.
use super::{Proof, addr, decoded, quantity};
use super::super::{levels::Depth, routing};
use crate::core::types::Side;
use crate::settings::recommendations::venue_value;
use alloy::rpc::types::Log;
use alloy_primitives::{Address, FixedBytes, U256};
use flate2::read::GzDecoder;
use rust_decimal::Decimal;
use serde_json::Value;
use std::collections::BTreeMap;

fn archive(data: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(data)).unwrap()
}

#[test]
fn base_profile_reconciles_the_paid_pair_in_both_directions() {
    let data = archive(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/native_rfq_base_wallets.json.gz"));
    let book = archive(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/native_rfq_base_book.json.gz"));
    let depth=Depth::parse(&book["rows"]).unwrap();
    let cash: Address=venue_value("dex.settlement_contract","base_dex").unwrap().parse().unwrap();
    assert_eq!(venue_value("venue.quote_asset","base_dex"),Some("USDC"));
    let mut rows: Vec<String>=venue_value("native_rfq.roles","base_dex").unwrap().split(',').map(str::to_owned).collect();
    let accepted=&data["observed"][1];
    rows.push(format!("module {} {}",accepted["module"].as_str().unwrap(),accepted["runtime"].as_str().unwrap()));
    let selected=super::super::profile::Profile::parse(&serde_json::json!(rows)).unwrap();
    assert!(selected.supports_settlement());
    rows[0]="network 1 ethereum".into();
    assert!(!super::super::profile::Profile::parse(&serde_json::json!(rows)).unwrap().supports_settlement());
    let payer=addr(&data["payer"]).unwrap();
    let pool=addr(&data["roles"][1]["address"]).unwrap();
    let mut groups=BTreeMap::new();
    for row in data["observed"].as_array().unwrap() {
        assert_eq!(row["status"],"settled");
        let proof=Proof {
            pool,payer,input:addr(&row["source"]).unwrap(),output:addr(&row["target"]).unwrap(),
            sold:quantity(&row["requested"]).unwrap(),minimum:quantity(&row["minimum"]).unwrap(),
            maker:addr(&row["maker"]).unwrap(),
            quote_id:FixedBytes::from_slice(&decoded(&row["quoteId"]).unwrap()),
            digest:row["offer_digest"].as_str().unwrap().parse().unwrap(),
        };
        let (side,coin)=if proof.input==cash {(Side::Buy,proof.output)} else {(Side::Sell,proof.input)};
        assert_eq!(routing::direction(coin,cash,side,false),Some((proof.input,proof.output)));
        assert!(routing::direction(coin,cash,side,true).is_none());
        assert!(depth.value(proof.input,proof.output,side,Decimal::new(1,4)).is_some());
        let logs: Vec<Log>=serde_json::from_value(row["logs"].clone()).unwrap();
        let module=addr(&row["module"]);
        let net=quantity(&row["net"]).unwrap();
        assert_eq!(proof.received(&logs,module),Some(net));
        assert_eq!(quantity(&row["before"][0]).unwrap()-quantity(&row["after"][0]).unwrap(),proof.sold);
        assert_eq!(quantity(&row["after"][1]).unwrap()-quantity(&row["before"][1]).unwrap(),net);
        if module.is_some() {
            assert_eq!(quantity(&row["strict"]), Some(net+U256::ONE));
            assert_eq!(row["module_before"],row["module_after"]);
            assert_eq!(row["rollback"][0],row["before"][0]);
            assert!(proof.received(&logs,Some(payer)).is_none());
        }
        *groups.entry((proof.input,proof.output,module.is_some())).or_insert(0)+=1;
    }
    assert_eq!(groups.len(),8);
    assert!(groups.values().all(|n|*n==2));
}
