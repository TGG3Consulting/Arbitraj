struct LiveShop<'a>{parts:&'a Parts,book:FrozenBook,rev:bool,conf:&'a Value}
impl LiveShop<'_>{
    fn d(&self,k:&str)->Decimal{num(self.conf[k].as_str().unwrap())}
}
impl Source for LiveShop<'_>{
    async fn quote(&self,_:&Direction,sell:bool,q:Decimal)->Result<Quote,Fail>{
        if sell==self.rev{return Ok(self.book.price(q));}
        let mut dq=shopinx::full(&self.parts.quoting(),if sell {Side::Sell}else{Side::Buy},q).await
            .map_err(|e|Fail::TooLarge(e.message))?;
        let mut i=0;
        for c in &mut dq.costs {
            if c.kind==CostKind::Gas{
                c.amount=num(self.conf["dex_gas"][i].as_str().unwrap());i+=1;
            }
        }
        assert_eq!(i,2);
        if self.d("approve_bnb")>Decimal::ZERO{dq.costs.push(Cost::new(CostKind::ApproveGas,self.d("approve_bnb"),"BNB",false));}
        Ok(dq)
    }
    fn worst(&self,_:&Direction,sell:bool,q:Decimal,avg:Decimal)->Result<Decimal,Fail>{
        if sell!=self.rev{return Ok(if sell{avg}else{avg*num("1.005")});}
        deepest(&self.book.book,if sell{Side::Sell}else{Side::Buy},q).ok_or_else(||Fail::TooLarge("not covered".into()))
    }
    async fn withdraw(&self,_:&DirCtx,_:Decimal)->Result<Vec<Cost>,Fail>{
        Ok(if self.rev{vec![Cost::new(CostKind::TransferGas,self.d("transfer_gas"),"BNB",false)]}
            else {vec![Cost::new(CostKind::WithdrawFee,self.d("withdraw"),"SHOPINX",false)]})
    }
    async fn ret(&self,_:&DirCtx,_:Decimal)->Result<Vec<Cost>,Fail>{
        Ok(if self.rev{vec![Cost::new(CostKind::UsdtReturn,self.d("ret"),"USDT",false)]}
            else{vec![Cost::new(CostKind::TransferGas,self.d("transfer_gas"),"BNB",false)]})
    }
    async fn rates(&self,cx:&DirCtx,coin:&str,coin_price:Decimal,_:&[Cost])->Result<Rates,Fail>{
        Ok(Rates{quote:cx.quote.clone(),coin:coin.into(),coin_price,prices:[("BNB".into(),self.d("bnb"))].into_iter().collect()})
    }
}
#[tokio::test]
async fn shop_live_control(){
    let raw:Value=serde_json::from_str(include_str!("shop_live_cases.json")).unwrap();
    for data in raw.as_array().unwrap(){
        let get=|key:&str|num(data[key].as_str().unwrap());
        let id=data["id"].as_u64().unwrap();let rev=data["reverse"].as_bool().unwrap();let block=data["block"].as_u64().unwrap();
        let words=&data["words"];let path=format!("tests/data/shop-live-{block}");let w=words[0].as_i64().unwrap() as i16;let v=words[1].as_i64().unwrap() as i16; let parts=if data["spacing"]==10 {shopinx::live_scene_other(&path,block,w,v,6).await}else if data["bridge"].as_str().unwrap().starts_with("0x172") {shopinx::live_scene_pcs(&path,block,w,v,6).await}else{shopinx::live_scene(&path,block,w,v,6).await};
        route_common::channel(&parts,"club48","bundle").await;route_common::executor(&parts,"0x0000000000000000000000000000000000000077").await;configure(&parts,"graph").await;shopinx::partial(&parts,Some(true)).await;
        let dir=Direction{asset:shopinx::shopinx(),buy_on:VenueId(if rev{"bsc_dex"}else{"bingx"}.into()),sell_on:VenueId(if rev{"bingx"}else{"bsc_dex"}.into())};
        let levels=|label:&str|data[label].as_array().unwrap().iter().map(|x|PriceLevel{qty:num(x[1].as_str().unwrap()),price:num(x[0].as_str().unwrap())}).collect::<Vec<_>>();
        let venue=VenueId("bingx".into());let now=Utc::now();let book=OrderBook{venue:venue.clone(),asset:dir.asset.clone(),received_at:now,exchange_time:None,asks:levels("asks"),bids:levels("bids")};
        let rule=serde_json::from_value(json!({"ticker":"SHOPINX","online":true,"status":1,"api_buy":true,"api_sell":true,"min_notional":"1","max_notional":"1029","step_size":"0.000001"})).unwrap();
        let mut cx=ctx(&dir,get("h"));cx.sigma=get("sigma");cx.transit_s=get("ts");cx.keep=Some(get("risk_keep"));cx.closure_share=None;
        if rev{cx.pair_b=Some(rule)}else{
            cx.pair_a=Some(rule);
            cx.a.info=Some(serde_json::from_value(json!({"venue":"bingx","asset":dir.asset,"network":"BEP20","deposit_enabled":true,"withdraw_enabled":true,"withdraw_fee":data["withdraw"],"withdraw_precision":8,"updated_at":now})).unwrap());
        };
        let mut ext=arb::core::size_extra::Extra::default();ext.min_usd=Some(get("threshold"));ext.min_pct=Some(get("min_pct"));ext.no_formula=true;ext.budget_monotone=!rev;
        if rev{ext.sell_regions=arb::core::size_scan::regions(&book.bids,cx.pair_b.as_ref());}
        let cum=|ls:&[PriceLevel]|{let mut q=Decimal::ZERO;ls.iter().map(|l|{q+=l.qty;q}).collect::<Vec<_>>()};
        let step=num("0.000001");let budget=get("budget");
        let p=Params{step,budget,guess:budget/book.asks[0].price,buy_breaks:if rev{vec![]}else{cum(&book.asks)},sell_breaks:if rev{cum(&book.bids)}else{vec![]},bisect_steps:40,grid_points:16,refine_steps:30,tolerance_pct:Decimal::ONE,max_evals:300,par_points:1,bisect_probes:3,early_floor:None,reuse:true,par_blocked:false,ext};
        let bs=if rev {Side::Sell}else{Side::Buy};
        let src=LiveShop{parts:&parts,book:FrozenBook{book,asset:dir.asset.clone(),venue,side:bs,taken_at:now,fee:BookFee::in_received(num("0.001"),bs,"SHOPINX","USDT")},conf:data,rev};
        let mut curve=SourceCurve{cx:&cx,src:&src};
        let repr=|c:arb::core::size::Point<arb::core::costs::Calc>|json!({"qty":c.qty,"net":c.profit,"spend":c.data.invested,"spend_max":c.data.spend_max,"avg":c.data.buy_avg,"sell_avg":c.data.sell_avg,"proceeds":c.data.proceeds,"costs":c.data.costs_usdt});
        let mut points=Vec::new();
        for q in [get("k_qty"),get("alt"),num(data["hint"]["qty"].as_str().unwrap()).max(step),num("20000"),num("25000"),num("28000"),num("30000"),num("32000"),num("34000"),num("40000")]{
            points.push(curve.at(q).await.ok().map(&repr));
        }
        let full_started=std::time::Instant::now(); let selected=search(&mut curve,&p).await.map(|f|json!({"point":repr(f.best),"evals":f.choice.evals,"hi":f.choice.max_qty,"cap":f.choice.capped})).ok();
        let full_us=full_started.elapsed().as_micros() as u64;
        use rust_decimal::MathematicalOps;
        let hc=&data["hint"]; let dec=|k:&str|num(hc[k].as_str().unwrap()); 
        let unit=dec("price_factor");let bend=dec("curve_factor");let bid=dec("price");let keep=cx.keep.unwrap();let pf=num("0.001");let depth=dec("first_level_size");let wd=get("withdraw");
        let formula=||if rev{(Decimal::ONE-(unit/((keep-pf)*bid)).sqrt().unwrap())/bend}
                    else{(((keep*unit*(Decimal::ONE-pf)/bid).sqrt().unwrap()-Decimal::ONE)/bend+wd)/(Decimal::ONE-pf)};
        let mut kernel=Vec::new();for _ in 0..63 {let t=std::time::Instant::now();std::hint::black_box(std::hint::black_box(&formula)().max(Decimal::ZERO).min(depth));kernel.push(t.elapsed().as_nanos() as u64);}kernel.sort();
        let mut single=Vec::new();for _ in 0..3 {let t=std::time::Instant::now();let _=std::hint::black_box(curve.at(get("k_qty")).await);single.push(t.elapsed().as_micros() as u64);}single.sort();
        let (mut lo,mut hi)=(num("200"),num("80000"));let mut probes=0;
        // Both pools have constant fee tiers, monotone marginal price between ticks; no liquidity gaps
        // may be assumed outside the captured range. This is a reference for this one frozen path.
        let mut best:Option<(Decimal,Decimal)>=None;
        for _ in 0..63 {
            if hi-lo < step*num("6"){break;}
            let a=((lo+(hi-lo)/num("3"))/step).floor()*step;let b=((hi-(hi-lo)/num("3"))/step).floor()*step;
            let mut pair=[None,None];
            for (i,n) in [a,b].into_iter().enumerate(){probes+=1;
                if let Ok(pt)=curve.at(n).await && pt.invested<=budget{
                    pair[i]=Some(pt.profit);let rank=(pt.profit,-n);if best.is_none_or(|v|rank>v){best=Some(rank);}
                }
            }
            if pair[0]>=pair[1]{hi=b}else{lo=a}
        }
        let reference=match best {Some((_,n))=>Some(repr(curve.at(-n).await.unwrap())),None=>None}; let source_error=curve.at(get("k_qty")).await.err().map(|e|format!("{e:?}"));
        println!("SHOP_LIVE {}",json!({"id":id,"block":block,"freeze":data["freeze"],"seq":data["seq"],"hint":data["hint"],"source_error":source_error,"full_us":full_us,"fresh_us_median3":single[1],"formula_ns_p50":kernel[31],"formula_ns_p95":kernel[59],"k_qty":get("k_qty"),"k_profit":get("net"),"archive_alt":get("alt"),"archive_alt_net":get("alt_net"),"points":points,"native":selected,"ref":reference,"ref_probes":probes,"ref_final_bracket":[lo,hi]}));
    }
}

