use alloy_primitives::{Address,U256};
use arb::core::types::Side;
use arb::venues::mexc::{order_rules::check_rules,symbols::SymbolRules};
use arb::venues::bsc_dex::math::v2;
use rust_decimal::Decimal;
use serde_json::{Value,json};
use std::{str::FromStr,fs,collections::HashSet};

fn inputs()->Value {serde_json::from_str(include_str!("wallet_audit_inputs.json")).unwrap()}
fn number(s:&str)->U256 {U256::from_str(s).unwrap()}
fn dec(s:&str)->Decimal {Decimal::from_str(s).unwrap()}
fn amount(v:&Value)->U256 {number(v.as_str().unwrap())}
fn addr(s:&str)->Address {Address::from_str(s).unwrap()}

#[test]
fn prices_and_order_guards() {
    let mut matrix=Vec::new();
    for v in inputs()["mboom"].as_array().unwrap(){
        let reserves=v["reserves"].as_array().unwrap();
        let quote=v2::amount_out(amount(&v["small"]),amount(&reserves[1]),amount(&reserves[0]),v2::fee_ratio(dec("0.25")).unwrap()).unwrap();
        assert_eq!(quote,amount(&v["quote"]));
        // Conditional symbol fixtures: absent historical exchangeInfo cannot be imputed with today's result.
        let ask=dec(v["ask"].as_str().unwrap());
        for threshold in ["1","5"] {
            let s=SymbolRules{symbol:"MBOOMUSDT".into(),step_size:dec("0.01"),tick_size:dec("0.000001"),min_qty:Some(dec("0.01")),min_notional:Some(dec(threshold)),max_notional:None,api_buy:true,api_sell:true,status:1,base_precision:2,quote_asset_precision:6,order_types:Some(vec!["LIMIT".into()]),/*PRICE_BAND_FIELD*/};
            let buy=dec("2887.77"); // Sell 2877.77 plus fixed withdrawal fee 10, buy fee paid in quote.
            let accepted=check_rules(&s,Side::Buy,buy,ask,"IMMEDIATE_OR_CANCEL").is_ok();
            assert_eq!(accepted,threshold=="1");
            matrix.push(json!({"id":v["id"],"q":buy,"price":ask,"simulated_usdt":quote.to_string(),"conditional_rule_usdt":threshold,"accepted":accepted}));
        }
    }
    let fixture=inputs();
    let response=json!({"symbols":[fixture["mexc_observed_25nov"]["symbol"]]});
    for authorized in [false,true] {
        let mut own=HashSet::new();
        if authorized {own.insert("MBOOMUSDT".to_string());}
        let parsed=arb::venues::mexc::symbols_parse::parse_list(&response,"USDT",&own).unwrap();
        let exact=&parsed["MBOOM"];
        assert_eq!(exact.min_notional, Some(Decimal::ONE));
        assert_eq!(exact.tick_size,dec("0.0000001"));
        for row in fixture["mboom"].as_array().unwrap(){
            let price=dec(row["ask"].as_str().unwrap());
            let response=check_rules(exact,Side::Buy,dec("2887.77"),price,"IMMEDIATE_OR_CANCEL");
            assert_eq!(response.is_ok(),authorized);
            matrix.push(json!({"id":row["id"],"symbol_data_time":fixture["mexc_observed_25nov"]["date"],"side":"BUY","type":"IMMEDIATE_OR_CANCEL","account_symbol_allowed_hypothesis":authorized,"result":response}));
        }
    }
    fs::write("case-orders.json",serde_json::to_vec_pretty(&matrix).unwrap()).unwrap();
}
