use alloy_primitives::U256;
use arb::venues::bsc_dex::{math::swap_cl::{self,ClState,Version,After},pools::ticks::{TickMap,TickData}};
use arb::venues::bsc_dex::{math::{ swap_dodo::{PmmState,RState,sell_base_token,sell_quote_token},v2},num::{to_decimal,to_units},pools::load_dodo::net_after_fees,quote::pool_quote::swap_fee};
fn uint(x:&Value)->Option<U256>{
    let string=x.as_str().map(str::to_owned).unwrap_or_else(||x.to_string());
    let (n,base)=string.strip_prefix("0x").map_or((string.as_str(),10),|hex|(hex,16));
    U256::from_str_radix(n,base).ok()
}
enum Hop {
    Pair {res_in:U256,res_out:U256,fee:u32},
    Tick {id:String,v4:bool,zin:bool,sqrt:U256,tick:i32,liq:u128,fee:u32,map:TickMap},
    Pmm {state:PmmState,base_input:bool,lp:U256,mt:U256},
}
impl Hop {
    fn parse(h:&Value)->Option<Self>{
        let fee=|v:&Value|u32::try_from(v.as_u64()?).ok();
        Some(match h["k"].as_str()? {
            "v2"=>Self::Pair{res_in:uint(&h["r_in"])?,res_out:uint(&h["r_out"])?,fee:fee(&h["fee"])?},
            "v3"|"cl"=>{
                let v4=h["k"]=="cl";let raw_fee=fee(&h["fee"])?;
                let mut ticks=std::collections::BTreeMap::new();
                for r in h["ticks"].as_array()? {
                    ticks.insert(r[0].as_i64()? as i32,TickData{gross:r[1].as_str()?.parse().ok()?,net:r[2].as_str()?.parse().ok()?});
                }
                Self::Tick{id:h["pool_id"].as_str()?.into(),v4,zin:h["zin"].as_bool()?,sqrt:uint(&h["sqrt"])?,tick:h["tick"].as_i64()? as i32,
                    liq:u128::try_from(uint(&h["liq"])?).ok()?,map:TickMap{spacing:h["spacing"].as_i64()? as i32,ticks:ticks.into(),words:(h["lo"].as_i64()? as i16,h["hi"].as_i64()? as i16)},
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
    fn result(&self, amount:U256,exact_in:bool,after:&mut std::collections::HashMap<String,After>)->Result<U256,&'static str>{
        match self {
            Self::Pair{res_in,res_out,fee}=>{
                let keep=1_000_000u32.checked_sub(*fee).ok_or("invalid V2 fee")?;
                let scale=(U256::from(keep),U256::from(1_000_000u32));
                (if exact_in{v2::amount_out}else{v2::amount_in})(amount,*res_in,*res_out,scale).ok_or("V2 reserve exhausted")
            }
            Self::Tick{id,v4,zin,sqrt,tick,liq,fee,map}=>{
                let current=after.get(id).copied().unwrap_or(After{sqrt_price:*sqrt,tick:*tick,liquidity:*liq});
                let state=ClState{sqrt_price:current.sqrt_price,tick:current.tick,liquidity:current.liquidity,fee:*fee,ticks:map,version:if *v4{Version::V4}else{Version::V3}};
                let (swap,post)=swap_cl::swap_final(&state,*zin,exact_in,amount)?;
                if !swap.complete {return Err("outside archive tick words")}
                let advanced=if exact_in {post} else {
                    let (forward,post)=swap_cl::swap_final(&state,*zin,true,swap.amount_in)?;
                    if !forward.complete{return Err("shared pool forward step outside archive words")}
                    post
                };
                after.insert(id.clone(),advanced);
                if exact_in && swap.amount_in==amount {Ok(swap.amount_out)}else if !exact_in && swap.amount_out==amount {Ok(swap.amount_in)}else{Err("incomplete pool fill")}
            }
            Self::Pmm{state,base_input,lp,mt}=>{
                if !exact_in{return Err("native DODO exact-output not supported")}
                let (gross,_)= (if *base_input{sell_base_token}else{sell_quote_token})(state,amount).ok_or("DODO pricing curve rejected input")?;
                net_after_fees(gross,*lp,*mt).ok_or("DODO commissions exceed gross")
            }
        }
    }
}
use arb::core::size::{Curve, Fail, Params, Point};
use arb::core::size_search::search;
use async_trait::async_trait;
use rust_decimal::Decimal;
use serde_json::{Value,json};
use std::{env,fs,str::FromStr};
struct Market { hops: Vec<Hop>, decimals:u8, fee:Decimal, usd:Decimal }
impl Market {
    fn point(&self,qty:Decimal)->Result<Point<Decimal>,Fail>{
        let mut raw=to_units(qty,self.decimals).ok_or_else(||Fail::Stop("units".into()))?;
        let mut states=std::collections::HashMap::new();
        for hop in &self.hops {raw=hop.result(raw,true,&mut states).map_err(|e|Fail::TooLarge(e.into()))?;}
        let out=to_decimal(raw,self.decimals).ok_or_else(||Fail::Stop("value".into()))?;
        Ok(Point{qty,invested:qty*self.usd,arrive:qty,profit:(out-qty-self.fee)*self.usd,data:out})
    }
}
#[async_trait]
impl Curve for Market {
    type Data=Decimal;
    async fn at(&mut self,qty:Decimal)->Result<Point<Self::Data>,Fail>{self.point(qty)}
}
fn dec(v:&Value)->Decimal{Decimal::from_str(v.as_str().unwrap()).unwrap()}
#[tokio::test]
async fn native_swap_and_exhaustive_cycle_size(){
    let input:Value=serde_json::from_slice(&fs::read(env::var("V2_INPUT").unwrap()).unwrap()).unwrap();
    let mut output=Vec::new();
    for x in input["cases"].as_array().unwrap() {
        let mut market=Market{hops:x["paths"].as_array().unwrap().iter().map(|h|Hop::parse(h).unwrap()).collect(),decimals:x["dec"].as_u64().unwrap()as u8,fee:dec(&x["fee"]),usd:dec(&x["usd"])};
        let cfg=&x["settings"];let cap=dec(&x["cap"]);let floor=dec(&cfg["dex_dex.min_profit_usd"]);
        let step=dec(&cfg["dex_dex.size_step_base"]);let num=|k:&str|cfg[k].as_u64().unwrap()as u32;
        assert!(cfg["size.pool_breaks_max"].is_null()&&cfg["size.early_exit_margin_usd"].is_null());
        let original=market.point(dec(&x["original"])).unwrap();assert_eq!(original.data,dec(&x["primary"]));
        let mut qty=step;let mut oracle=market.point(step).unwrap();let mut visited=0;
        while qty*market.usd<=cap{let p=market.point(qty).unwrap();if p.profit>oracle.profit{oracle=p;}qty+=step;visited+=1;}
        let mut arms=Vec::new();
        for budget in [num("size.max_evals"),5000] {
            let settings=Params{step,budget:cap,guess:cap/market.usd,buy_breaks:vec![],sell_breaks:vec![],bisect_steps:num("size.bisect_steps"),grid_points:num("size.grid_points"),refine_steps:num("size.refine_steps"),max_evals:budget,tolerance_pct:dec(&cfg["size.tolerance_pct"]),par_points:num("size.parallel_points"),bisect_probes:num("size.bisect_probes"),early_floor:None,reuse:false,par_blocked:false,ext:arb::core::size_extra::Extra{min_usd:Some(floor),budget_monotone:true,..Default::default()}};
            let f=search(&mut market,&settings).await.unwrap();
            arms.push(json!({"eval_limit":budget,"qty":f.best.qty.to_string(),"profit":f.best.profit.to_string(),"evaluations":f.choice.evals,"admit":f.best.profit>=floor}));
        }
        output.push(json!({"k":x["k"],"original_output":original.data.to_string(),"original_profit":original.profit.to_string(),"archive_matches":original.data==dec(&x["archived"]),"steps":visited,"oracle_qty":oracle.qty.to_string(),"oracle_profit":oracle.profit.to_string(),"native":arms}));
    }
    fs::write(env::var("V2_OUT").unwrap(),serde_json::to_vec(&output).unwrap()).unwrap();
}
