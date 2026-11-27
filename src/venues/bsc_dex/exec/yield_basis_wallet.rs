//! Locks receipt reconstruction to the signed factory market and net wallet movements.
use super::{fill,signed_call};
use crate::{gateway::errors::{ErrorCode,GatewayResult},venues::bsc_dex::{
    check::simulate::SimLog,ctx::Ctx,pools::{solidly_classic_factory,yield_basis},
    quote::v2_wallet_sim::Order,send::track::Receipt}};
use alloy::{rpc::types::Log,sol_types::{SolCall,SolEvent}};
use alloy_primitives::{Address,B256,Bytes,U256};
use serde::{Deserialize,Serialize};

#[allow(clippy::too_many_arguments)]
pub(in crate::venues::bsc_dex) mod abi {
    alloy::sol! {
        interface IYieldWallet {
            struct WalletOrder {bytes32 market;address source;address target;uint256 amount;uint256 minimum;uint256 deadline;}
            function execute(WalletOrder order) external payable returns(uint256);
            function factory() external view returns(address);
            function wrapped() external view returns(address);
            event Paid(address indexed payer,address indexed recipient,address indexed venue,address source,address target,uint256 spent,uint256 received);
        }
        interface IUnderlyingSwap {
            event TokenExchange(address indexed buyer,uint256 sold,uint256 spent,uint256 bought,uint256 received,uint256 fee,uint256 price);
        }
        interface IVirtualSwap {
            event TokenExchange(address indexed buyer,uint256 sold,uint256 spent,uint256 bought,uint256 received);
        }
    }
}
use abi::{IUnderlyingSwap,IYieldWallet,IVirtualSwap};

#[derive(Debug,Clone,Copy,Serialize,Deserialize)]
pub struct Terms {
    pub market:B256,
    pub target:Address,
    pub stable:Address,
    pub wrapped:Address,
    pub native:bool,
    pub module_hash:B256,
}
impl Terms {
    fn asset(&self,token:Address)->Address {
        if self.native && token==self.wrapped {Address::ZERO} else {token}
    }
    pub fn value(&self,input:Address,amount:U256)->U256 {
        if self.asset(input).is_zero(){amount}else{U256::ZERO}
    }
    pub(in crate::venues::bsc_dex) fn proof(self,o:&Order,minimum:U256,deadline:U256)->Proof {
        Proof{terms:self,payer:o.payer,input:o.input,output:o.output,maximum:o.amount,minimum,deadline}
    }
}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct Proof {
    pub terms:Terms, pub payer:Address, pub input:Address, pub output:Address,
    pub maximum:U256, pub minimum:U256, pub deadline:U256,
}
fn unique<E:SolEvent>(logs:&[Log],contract:Address)->Option<E> {
    let mut hits=logs.iter().filter(|l|l.address()==contract && l.topic0()==Some(&E::SIGNATURE_HASH));
    let event=E::decode_log_data(&hits.next()?.inner.data).ok()?;
    hits.next().is_none().then_some(event)
}
impl Proof {
    pub fn data(&self)->Bytes {
        let t=&self.terms;
        IYieldWallet::executeCall{order:IYieldWallet::WalletOrder{market:t.market,source:t.asset(self.input),
            target:t.asset(self.output),amount:self.maximum,minimum:self.minimum,deadline:self.deadline}}.abi_encode().into()
    }
    pub(super) async fn included(&self,ctx:&Ctx,hash:B256,r:&Receipt)->GatewayResult<()> {
        let t=&self.terms;
        signed_call::included(ctx,hash,r,self.payer,t.target,t.value(self.input,self.maximum),self.data()).await?;
        let block=r.block_hash.ok_or_else(||ctx.err(ErrorCode::OutcomeUnknown,"YieldBasis: отсутствует блок квитанции"))?;
        let at=serde_json::json!({"blockHash":block,"requireCanonical":true});
        solidly_classic_factory::plain(ctx,t.target,t.module_hash,&at).await?;
        crate::venues::bsc_dex::send::canonical_receipt::verify(ctx,hash,r).await
    }
    pub fn measured(&self,logs:&[SimLog])->Option<(U256,U256)> {
        self.amounts(&signed_call::logs(logs)?,self.payer,self.input,self.output)
    }
    pub fn amounts(&self,logs:&[Log],payer:Address,input:Address,output:Address)->Option<(U256,U256)> {
        let t=&self.terms;
        let (_,role,venue)=yield_basis::key(t.market)?;
        if (payer,input,output)!=(self.payer,self.input,self.output) || input==output || self.minimum.is_zero() ||
            (input!=t.stable && output!=t.stable) {return None;}
        let paid=unique::<IYieldWallet::Paid>(logs,t.target)?;
        if (paid.payer,paid.recipient,paid.venue,paid.source,paid.target)!=(payer,payer,venue,t.asset(input),t.asset(output)) ||
            paid.spent!=self.maximum || paid.spent.is_zero() || paid.received<self.minimum {return None;}
        let (buyer,sold,spent,bought,received)=if role==2 {
            let e=unique::<IVirtualSwap::TokenExchange>(logs,venue)?;
            (e.buyer,e.sold,e.spent,e.bought,e.received)
        }else{
            let e=unique::<IUnderlyingSwap::TokenExchange>(logs,venue)?;
            (e.buyer,e.sold,e.spent,e.bought,e.received)
        };
        // The virtual pool indexes the beneficiary. Both other contracts index msg.sender.
        let origin=if role==2 && !paid.target.is_zero(){payer}else{t.target};
        let i=U256::from(u8::from(input!=t.stable));
        if (buyer,sold,bought,spent)!=(origin,i,U256::ONE-i,paid.spent) || received<paid.received ||
            fill::moved_logs(logs,input,Some(t.target),Some(venue))!=paid.spent {return None;}
        let credits=|token|fill::moved_logs(logs,token,None,Some(payer));
        let debits=|token|fill::moved_logs(logs,token,Some(payer),None);
        if !paid.source.is_zero() && debits(input).checked_sub(credits(input))?!=paid.spent {return None;}
        if !paid.target.is_zero() && credits(output).checked_sub(debits(output))?!=paid.received {return None;}
        Some((paid.spent,paid.received))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn recorded_wallet_routes_match_the_original_market_and_reject_a_different_payment() {
        let dir=include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/yield_basis_ethereum.json.gz");
        let paid=include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/yield_basis_wallet.json.gz");
        let book:Value=serde_json::from_reader(flate2::read::GzDecoder::new(dir.as_slice())).unwrap();
        let wallet:Value=serde_json::from_reader(flate2::read::GzDecoder::new(paid.as_slice())).unwrap();
        let address=|v:&Value|v.as_str().unwrap().parse::<Address>().unwrap();
        let units=|v:&Value|v.as_str().unwrap().parse::<U256>().unwrap();
        let wrapped="0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2".parse::<Address>().unwrap();
        let payer=Address::from_slice(&alloy_primitives::keccak256("Arbitraj Base 8453 isolated owner 2026-11-26")[12..]);
        let mut checked=0;
        for row in wallet["cases"].as_array().unwrap().iter().filter(|r|r.get("logs").is_some() && r["mode"]!="v5"){
            let mut key=[0u8;32];
            key[3..11].copy_from_slice(&row["index"].as_u64().unwrap().to_be_bytes());
            key[11]=match row["kind"].as_str().unwrap(){"cryptopool"=>0,"amm"=>1,"virtual_pool"=>2,_=>panic!("unknown market")};
            key[12..].copy_from_slice(address(&row["venue"]).as_slice());
            let underlying=|asset:Address|if asset.is_zero(){wrapped}else{asset};
            let input=underlying(address(&row["source"]));
            let output=underlying(address(&row["target"]));
            let terms=Terms{market:B256::from(key),target:address(&wallet["module"]),stable:address(&book["stable"]),wrapped,
                            native:row["mode"]=="native",module_hash:B256::ZERO};
            let mut proof=Proof{terms,payer,input,output,maximum:units(&row["spent"]),minimum:units(&row["received"]),deadline:U256::MAX};
            let logs:Vec<SimLog>=serde_json::from_value(row["logs"].clone()).unwrap();
            assert_eq!(proof.measured(&logs),Some((proof.maximum,proof.minimum)));
            proof.minimum+=U256::ONE;
            assert_eq!(proof.measured(&logs),None);
            proof.minimum-=U256::ONE;
            proof.terms.market=B256::ZERO;
            assert_eq!(proof.measured(&logs),None);
            checked+=1;
        }
        assert_eq!(checked,10);
    }
}
