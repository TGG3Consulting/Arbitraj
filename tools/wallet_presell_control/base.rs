use alloy_primitives::{Address,U256};
use arb::core::types::Side;
use arb::venues::mexc::{order_rules::check_rules,symbols::SymbolRules};
use arb::venues::bsc_dex::math::v2;
use rust_decimal::Decimal;
use serde_json::{Value,json};
use std::{str::FromStr,fs};

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
            let accepted=check_rules(&s,Side::Buy,buy,ask,"LIMIT").is_ok();
            assert_eq!(accepted,threshold=="1");
            matrix.push(json!({"id":v["id"],"q":buy,"price":ask,"simulated_usdt":quote.to_string(),"conditional_rule_usdt":threshold,"accepted":accepted}));
        }
    }
    fs::write("case-orders.json",serde_json::to_vec_pretty(&matrix).unwrap()).unwrap();
}
