#[tokio::test]
async fn shop_depth_control(){
    use rust_decimal::RoundingStrategy;
    let data:Value=serde_json::from_str(include_str!("shop_live_cases.json")).unwrap();
    let c=data.as_array().unwrap().iter().find(|c|c["id"]==31422).unwrap();
    let qty=num(c["k_qty"].as_str().unwrap());
    let after=qty*(Decimal::ONE-num("0.001"))-num("121");
    let deep=after/num("0.7");
    let ceil=deep.round_dp_with_strategy(18,RoundingStrategy::ToPositiveInfinity);
    let unit=num("0.000000000000000001");
    for w in [6i16,12]{ 
        let p=shopinx::live_scene("tests/data/shop-live-134331649",134331649,-463,-250,w).await;
        route_common::channel(&p,"club48","bundle").await;
        route_common::executor(&p,"0x0000000000000000000000000000000000000077").await;
        for mode in ["classic","graph"]{
            configure(&p,mode).await;shopinx::partial(&p,Some(true)).await;
            let q=p.quoting();
            for (tag,a) in [("trade",after),("floor",ceil-unit),("depth",deep),("ceil",ceil),("ceil+1",ceil+unit),("huge",num("99999999"))]{
                let f=q.fills(&shopinx::shopinx(),Side::Sell,a).await.map_err(|e|e.message);
                let priced=q.priced(&shopinx::shopinx(),Side::Sell,a).await.map_err(|e|e.message);
                let detail=priced.map(|z|json!({"fill":z.fillable_qty,"avg":z.avg_price,"route":z.route}));
                let compact=|r:Result<Decimal,String>|r.map(|v|json!({"value":v,"delta":v-a,"covers":v>=a,"covers_original":v>=deep}));
                println!("SHOP_DEPTH {}",json!({"case":31422,"width":w,"router":mode,"label":tag,"qty":qty,"net_token":after,"a":a,"depth":deep,"fills":compact(f),"priced":detail}));
            }
        }
    }
}
