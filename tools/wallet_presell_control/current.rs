use arb::venues::bsc_dex::exec::dry_fund_gather::{gather,Pick,net,qty_share};
#[test]
fn current_donor_selection() {
    let mut output=Vec::new();
    for row in inputs()["mboom"].as_array().unwrap(){
        let holder=[(addr("0x28e2ea090877bf75740558f6bfb36a5ffee9e9df"),amount(&row["balance"]))];
        let want=amount(&row["wanted"]);
        let cap=gather(&[&[],&holder],want,None).unwrap_err();assert_eq!(cap,amount(&row["balance"]));
        let need=if row["id"]==171 {dec("18132.71")}else{dec("124740.49")};
        let debit=net(cap,None).unwrap()-U256::from(2);
        let max=qty_share(need,debit,want,18).unwrap();assert!(max>=dec("2877.77")&&max<dec("2877.78"));
        assert_eq!(gather(&[&[],&holder],amount(&row["small"]),None),Ok(Pick::Direct(holder[0].0)));
        output.push(json!({"id":row["id"],"full_eligible":false,"max_sell":max,"smaller_eligible":true}));
    }
    let fx=inputs();
    let pools:Vec<_>=fx["donors"].as_array().unwrap().iter().map(|d|(addr(d["address"].as_str().unwrap()),amount(&d["raw"]))).collect();
    let total=gather(&[&pools],number("107985845390000000000000"),None).unwrap_err();
    assert_eq!(total,number("7147974183670266786116"));
    let costs=arb::core::types::TokenTaxes{buy_pct:Decimal::ZERO,transfer_pct:Decimal::ZERO,sell_pct:dec("10")};
    let with_tax=gather(&[&pools],number("107985845390000000000000"),Some(&costs)).unwrap_err();
    let expected=amount(&fx["donors"][0]["raw"])+pools[1..].iter().map(|(_,v)|*v).sum::<U256>()*U256::from(9)/U256::from(10);
    assert_eq!(with_tax,expected);
    assert_eq!(net(with_tax,Some(&costs)),Some(with_tax));
    let journal_costs=arb::core::types::TokenTaxes{sell_pct:dec("10.0006"),..costs};
    let journal_cap=gather(&[&pools],number("126591407630000000000000"),Some(&journal_costs)).unwrap_err();
    assert!(journal_cap<with_tax);
    let floor_sell=dec("11330")*dec("0.99901")-dec("1344.81");assert!(floor_sell>dec("7147.975"));
    output.push(json!({"id":221,"max_before_transfer_tax_and_margin_raw":total.to_string(),"source_gather_with_10pct_sell_tax_raw":with_tax.to_string(),"source_gather_with_logged_10_0006pct_raw":journal_cap.to_string(),"observed_floor_after_fees":floor_sell}));
    fs::write("case-presell.json",serde_json::to_vec_pretty(&output).unwrap()).unwrap();
}
