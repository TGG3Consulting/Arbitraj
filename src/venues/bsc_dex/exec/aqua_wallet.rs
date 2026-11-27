//! Поручение включает полные байты программы, способ оплаты, две валюты, минимум, получателя и срок.
use super::{fill,signed_call};
use crate::venues::bsc_dex::{pools::aqua::{self,IAqua,Program},quote::v2_wallet_sim::Order};
use alloy::{rpc::types::Log,sol_types::{SolCall,SolEvent}};
use alloy_primitives::{Address,B256,Bytes,U256,keccak256};
use serde::{Deserialize,Serialize};
alloy::sol! {
    interface IGuard {
        struct Limits {address input;address output;uint256 amount;uint256 minimum;uint256 deadline;bool inputFirst;}
        function execute(Program order,Limits terms) external returns(uint256);
        function preview(Program order,Limits terms,address recipient) external view returns(uint256);
        function engine() external view returns(address);
        function registry() external view returns(address);
        function v5Code() external view returns(bytes32);
        function orderForKey(bytes32 key) external view returns(bytes32);
        event Settled(bytes32 indexed orderHash,address indexed payer,address indexed recipient,address input,address output,uint256 paid,uint256 received,bool inputFirst);
    }
    interface ISwap {
        function swap(Program order,address src,address dst,uint256 amount,bytes taker) external returns(uint256,uint256,bytes32);
    }
}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct Terms{pub chain:u64,pub target:Address,pub engine:Address,pub maker:Address,pub hash:B256,pub raw:Bytes,pub first:bool}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct Proof{
    pub terms:Terms,pub payer:Address,pub input:Address,pub output:Address,
    pub amount:U256,pub minimum:U256,pub deadline:U256,
}
impl Terms{
    pub(in crate::venues::bsc_dex) fn proof(&self,o:&Order,minimum:U256,deadline:U256)->Proof{
        Proof{terms:self.clone(),payer:o.payer,input:o.input,output:o.output,amount:o.amount,minimum,deadline}
    }
}
impl Proof{
    pub fn data(&self)->Bytes{
        let t=&self.terms;
        let Some(order)=aqua::program(t.maker,t.hash,&t.raw) else{return Bytes::new()};
        if t.target==t.engine{
            let Some(taker)=u64::try_from(self.deadline).ok().and_then(|deadline|aqua::traits(self.minimum,self.payer,deadline,t.first)) else{return Bytes::new()};
            return ISwap::swapCall{order,src:self.input,dst:self.output,amount:self.amount,taker}.abi_encode().into();
        }
        IGuard::executeCall{order,terms:IGuard::Limits{input:self.input,output:self.output,amount:self.amount,
            minimum:self.minimum,deadline:self.deadline,inputFirst:t.first}}.abi_encode().into()
    }
    pub(super) fn signed(&self)->signed_call::Erc20Call{
        signed_call::Erc20Call{chain:self.terms.chain,payer:self.payer,module:self.terms.target,calldata:keccak256(self.data())}
    }
    pub(in crate::venues::bsc_dex) fn measured(&self,logs:&[crate::venues::bsc_dex::check::simulate::SimLog])->Option<(U256,U256)>{
        self.amounts(&signed_call::logs(logs)?,self.payer,self.input,self.output)
    }
    pub(super) fn amounts(&self,logs:&[Log],payer:Address,input:Address,output:Address)->Option<(U256,U256)>{
        let t=&self.terms;
        if (payer,input,output)!=(self.payer,self.input,self.output) || input==output{return None;}
        let guarded=t.target!=t.engine;
        let taker=if guarded{t.target}else{payer};
        let mut swaps=logs.iter().filter(|l|l.address()==t.engine&&l.topic0()==Some(&IAqua::Swapped::SIGNATURE_HASH))
            .filter_map(|l|IAqua::Swapped::decode_log_data(&l.inner.data).ok()).filter(|s|s.orderHash==t.hash&&s.taker==taker);
        let s=swaps.next()?;
        if swaps.next().is_some() || (s.maker,s.tokenIn,s.tokenOut,s.amountIn)!=(t.maker,input,output,self.amount){return None;}
        let cash=|token,from,to|fill::moved_logs(logs,token,from,to);
        let paid=cash(input,Some(payer),None).checked_sub(cash(input,None,Some(payer)))?;
        let net=cash(output,None,Some(payer)).checked_sub(cash(output,Some(payer),None))?;
        if paid!=self.amount || net.is_zero() || net<self.minimum || cash(input,Some(payer),Some(t.target))!=paid
            || cash(output,Some(t.maker),Some(payer))!=net {return None;}
        if guarded{
            let mut transfers=logs.iter().filter(|l|l.address()==t.target && l.topic0()==Some(&IGuard::Settled::SIGNATURE_HASH));
            let f=IGuard::Settled::decode_log_data(&transfers.next()?.inner.data).ok()?;
            if transfers.next().is_some() || (f.orderHash,f.payer,f.recipient,f.input,f.output)!=(t.hash,payer,payer,input,output)
                || (f.paid,f.received,f.inputFirst)!=(paid,net,t.first){return None;}
        }else if net!=s.amountOut{return None;}
        Some((paid,net))
    }
}
