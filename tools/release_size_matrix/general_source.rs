use alloy_primitives::{I256, U256};
use arb::core::{convert::Rates, costs_source::Source, size::Fail, types::{Cost, Quote}, uncapped_replay_plan::dex_quote};
use arb::venues::bsc_dex::{math::{swap_step::{step_v3, step_v4}, swap_dodo::{PmmState,RState,sell_base_token,sell_quote_token},v2},num::{to_decimal,to_units},pools::load_dodo::net_after_fees,quote::pool_quote::swap_fee};
fn uint(x:&Value)->Option<U256>{
    let string=x.as_str().map(str::to_owned).unwrap_or_else(||x.to_string());
    let (n,base)=string.strip_prefix("0x").map_or((string.as_str(),10),|hex|(hex,16));
    U256::from_str_radix(n,base).ok()
}
enum Hop {
    Pair {res_in:U256,res_out:U256,fee:u32},
    Tick {v4:bool,sqrt:U256,boundary:U256,liq:u128,fee:u32},
    Pmm {state:PmmState,base_input:bool,lp:U256,mt:U256},
}
impl Hop {
    fn parse(h:&Value)->Option<Self>{
        let fee=|v:&Value|u32::try_from(v.as_u64()?).ok();
        Some(match h["k"].as_str()? {
            "v2"=>Self::Pair{res_in:uint(&h["r_in"])?,res_out:uint(&h["r_out"])?,fee:fee(&h["fee"])?},
            "v3"|"cl"=>{
                let v4=h["k"]=="cl";let raw_fee=fee(&h["fee"])?;
                Self::Tick{v4,sqrt:uint(&h["sqrt"])?,boundary:uint(&h["boundary"])?,
                    liq:u128::try_from(uint(&h["liq"])?).ok()?,
                    fee:if v4{swap_fee(fee(&h["protocol"])?,raw_fee,h["zin"].as_bool()?)}else{raw_fee}}
            }
            "pmm"=>{
                let v=&h["pmm"];let r=match v["r"].as_u64()?{0=>RState::One,1=>RState::AboveOne,2=>RState::BelowOne,_=>return None};
                Self::Pmm{state:PmmState{i:uint(&v["i"])?,k:uint(&v["k"])?,b:uint(&v["b"])?,q:uint(&v["q"])?,b0:uint(&v["b0"])?,q0:uint(&v["q0"])?,r},
                    base_input:h["base_in"].as_bool()?,lp:uint(&v["lp_fee_rate"])?,mt:uint(&v["mt_fee_rate"])?}
            }
            _=>return None,
        })
    }
    fn result(&self, amount:U256,exact_in:bool)->Result<U256,&'static str>{
        match self {
            Self::Pair{res_in,res_out,fee}=>{
                let keep=1_000_000u32.checked_sub(*fee).ok_or("invalid V2 fee")?;
                let scale=(U256::from(keep),U256::from(1_000_000u32));
                (if exact_in{v2::amount_out}else{v2::amount_in})(amount,*res_in,*res_out,scale).ok_or("V2 reserve exhausted")
            }
            Self::Tick{v4,sqrt,boundary,liq,fee}=>{
                let positive=I256::try_from(amount).map_err(|_|"signed amount overflow")?;
                let signed=if exact_in != *v4{positive}else{-positive};
                let (next,input,output,charge)=(if *v4{step_v4}else{step_v3})(*sqrt,*boundary,*liq,signed,*fee).ok_or("invalid or missing tick state")?;
                if next<=*sqrt.min(boundary) || next>=*sqrt.max(boundary){return Err("next initialized tick and liquidity are not recorded")}
                let spent=input.checked_add(charge).ok_or("fee overflow")?;
                if exact_in && spent==amount{Ok(output)}else if !exact_in && output==amount{Ok(spent)}else{Err("incomplete pool fill")}
            }
            Self::Pmm{state,base_input,lp,mt}=>{
                if !exact_in{return Err("native DODO exact-output not supported")}
                let (gross,_)= (if *base_input{sell_base_token}else{sell_quote_token})(state,amount).ok_or("DODO pricing curve rejected input")?;
                net_after_fees(gross,*lp,*mt).ok_or("DODO commissions exceed gross")
            }
        }
    }
}
struct Leg{share:Decimal,hops:Vec<Hop>}
struct Frozen {
    book_replay:Replay, dex_quote:Quote, legs:Vec<Leg>, sale:bool, coin:u8, stable:u8, book_depth:Decimal,
}
impl Frozen{
    fn amounts(&self,qty:Decimal)->Result<Vec<Decimal>,&'static str>{
        let mut result=Vec::with_capacity(self.legs.len());
        for leg in &self.legs{
            let part=qty.checked_mul(leg.share).ok_or("share overflow")?;
            let mut value=to_units(part,self.coin).ok_or("quantity cannot be represented in token units")?;
            if value.is_zero(){return Err("zero-size leg")}
            if self.sale {for h in &leg.hops {value=h.result(value,true)?;}}
            else {for h in leg.hops.iter().rev(){value=h.result(value,false)?;}}
            result.push(to_decimal(value,self.stable).ok_or("quote amount outside decimal range")?);
        }
        Ok(result)
    }
}
impl Source for Frozen{
    async fn quote(&self,d:&Direction,sell:bool,qty:Decimal)->Result<Quote,Fail>{
        if sell!=self.sale{
            if qty>self.book_depth{return Err(Fail::TooLarge("beyond archived CEX depth".into()))}
            return self.book_replay.quote(d,sell,qty).await;
        }
        let amounts=self.amounts(qty).map_err(|e|Fail::TooLarge(e.into()))?;
        let mut q=dex_quote(&self.dex_quote,qty);
        let total:Decimal=amounts.iter().sum();
        if total<=Decimal::ZERO || qty<=Decimal::ZERO{return Err(Fail::TooSmall("empty return".into()))}
        q.avg_price=total/qty;
        if let Some(route)=&mut q.route {
            for ((row,leg),out) in route.legs.iter_mut().zip(&self.legs).zip(amounts) {
                row.expected_out=if sell{out}else{qty*leg.share};
            }
        }
        Ok(q)
    }
    fn worst(&self,d:&Direction,sell:bool,qty:Decimal,avg:Decimal)->Result<Decimal,Fail>{
        if sell==self.sale{Ok(avg)}else{self.book_replay.worst(d,sell,qty,avg)}
    }
    async fn withdraw(&self,c:&DirCtx,q:Decimal)->Result<Vec<Cost>,Fail>{self.book_replay.withdraw(c,q).await}
    async fn ret(&self,c:&DirCtx,q:Decimal)->Result<Vec<Cost>,Fail>{self.book_replay.ret(c,q).await}
    async fn rates(&self,c:&DirCtx,coin:&str,price:Decimal,must:&[Cost])->Result<Rates,Fail>{self.book_replay.rates(c,coin,price,must).await}
}
async fn replay(case:&Value,budget:Decimal,max_evals:u32)->Result<Value,String>{
    let mut direction=case["direction"].clone();let mut raw_plan=case["plan"].clone();
    convert_asset(&mut direction["asset"]);convert_asset(&mut raw_plan["buy_quote"]["asset"]);convert_asset(&mut raw_plan["sell_quote"]["asset"]);
    let dir:Direction=serde_json::from_value(direction).map_err(|e|e.to_string())?;
    let plan=Plan::parse(&raw_plan)?;let sale=case["sell_dex"].as_bool().ok_or("historical DEX side")?;
    let book=read_book(&case["book"],&dir,sale)?;
    let empty=OrderBook{venue:if sale{dir.sell_on.clone()}else{dir.buy_on.clone()},asset:dir.asset.clone(),bids:vec![],asks:vec![],exchange_time:None,received_at:book.received_at};
    let (buy,sell)=if sale{(book.clone(),empty)}else{(empty,book.clone())};
    let legs=case["paths"].as_array().ok_or("missing split")?.iter().map(|leg|{
        let hs=leg["hops"].as_array().ok_or("missing hops")?.iter().map(|h|Hop::parse(h).ok_or("unknown hop fields")).collect::<Result<_,_>>()?;
        Ok(Leg{share:decimal(&leg["share"])?,hops:hs})
    }).collect::<Result<Vec<_>,String>>()?;
    let cfg=&case["control"];let floor=(decimal(&cfg["money.min_profit_usd"])?,decimal(&cfg["money.min_profit_pct"])?);
    let cx=context(&dir,&plan,decimal(&cfg["profit.price_move_k"])?,decimal(&cfg["profit.liquidity_share_h"])?,cfg["profit.closure_share"].as_str().map(d));
    let frozen=Frozen{book_replay:Replay::new(&dir,&plan,buy.clone(),if sale{None}else{Some(sell.clone())},["USDT","USDT"])?,
        dex_quote:if sale{plan.sell_quote.clone()}else{plan.buy_quote.clone()},legs,sale,stable:case["stable_digits"].as_u64().ok_or("missing quote decimals")? as u8,coin:case["coin_digits"].as_u64().ok_or("missing token decimals")? as u8,
        book_depth:if sale{&book.asks}else{&book.bids}.iter().map(|l|l.qty).sum()};
    let observed=if sale{plan.sell_qty}else{plan.qty};let amounts=frozen.amounts(observed)?;
    if sale {
        for (leg,amount) in case["paths"].as_array().unwrap().iter().zip(&amounts){
            if decimal(&leg["original_out"])?!=*amount{return Err("native original leg output differs from receipt/plan raw quantity".into())}
        }
    } else {
        let expected=observed*frozen.dex_quote.avg_price;let actual:Decimal=amounts.iter().sum();
        let tol=Decimal::new(2*frozen.legs.iter().map(|l|l.hops.len() as i64).sum::<i64>(),u32::from(frozen.stable));
        if (actual-expected).abs()>tol{return Err("native original exact-output cost differs from recorded price beyond rounding".into())}
    }
    let mut curve=SourceCurve{cx:&cx,src:&frozen};let own=curve.at(plan.qty).await.map_err(|e|format!("recorded quantity fails cost calculation: {e:?}"))?;
    // The standard search helper takes a CEX buyer as its reference. For DEX purchases
    // obtain the historical initial guess from the DEX quote and leave buyer breakpoints empty.
    let mut proxy=buy.clone();if !sale{proxy.asks.push(PriceLevel{price:plan.buy_quote.avg_price,qty:plan.qty});}
    let mut par=params(&plan,&proxy,&sell,budget,floor,max_evals);
    if !sale {par.buy_breaks.clear();let digits=book.bids.iter().map(|l|l.qty.normalize().scale()).fold(plan.qty.normalize().scale(),u32::max).min(18);par.step=Decimal::new(1,digits);}
    par.guess=par.guess.min(frozen.book_depth);
    let result=search(&mut curve,&par).await;let best=result.as_ref().ok().map(|f|&f.best);
    Ok(json!({"own_qty":plan.qty,"same_inputs_own_profit_usdt":own.profit,"selected_qty":best.map(|p|p.qty),"selected_spend_max_usdt":best.map(|p|p.invested),"selected_profit_usdt":best.map(|p|p.profit),"passes":best.is_some_and(|p|passes(&p.data,floor.0,floor.1)),"search_error":result.as_ref().err().map(|e|format!("{e:?}"))}))
}
