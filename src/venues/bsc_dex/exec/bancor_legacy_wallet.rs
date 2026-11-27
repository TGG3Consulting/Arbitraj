//! Signed v2.1 conversion bound to the original anchor and the recipient's final balance.
use super::{fill, signed_call};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{check::simulate::SimLog, ctx::Ctx, pools::solidly_classic_factory,
                     quote::v2_wallet_sim::Order, send::track::Receipt},
};
use alloy::{rpc::types::Log, sol_types::{SolCall, SolEvent, SolValue}};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};

pub(crate) const NATIVE: Address = alloy_primitives::address!("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
#[allow(clippy::too_many_arguments)]
pub(in crate::venues::bsc_dex) mod abi {
    alloy::sol! {
        interface ILegacyWallet {
            struct WalletOrder {address source;address target;uint256 amount;uint256 minimum;uint256 deadline;address[] path;}
            function execute(WalletOrder order) external payable returns(uint256);
            function wrapped() external view returns(address);
            function pools() external view returns(address);
            function network() external view returns(address);
            event Paid(address indexed payer,address indexed recipient,bytes32 indexed pathHash,address source,address target,uint256 spent,uint256 received);
        }
        interface ILegacyNetwork {
            function owner() external view returns(address);
            function rateByPath(address[] path,uint256 amount) external view returns(uint256);
            event Conversion(address indexed anchor,address indexed source,address indexed target,uint256 sourceAmount,uint256 targetAmount,address trader);
        }
    }
}
use abi::{ILegacyNetwork, ILegacyWallet};

#[derive(Debug,Clone,Copy,Eq,PartialEq,Serialize,Deserialize)]
pub struct Bridge {
    pub middle: Address,
    pub anchor: Address,
    pub converters: [Address;2],
}

#[derive(Debug,Clone,Copy,Serialize,Deserialize)]
pub struct Terms {
    pub target: Address,
    pub network: Address,
    pub normalized: [Address;2],
    pub reserves: [Address;2],
    pub anchor: Address,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub bridge: Option<Bridge>,
    pub native: bool,
    pub module_hash: B256,
}
impl Terms {
    pub fn path(&self,input:Address)->Option<Vec<Address>> {
        let i=self.normalized.iter().position(|t| *t==input)?;
        let mut path=vec![self.reserves[0],self.anchor];
        if let Some(b)=self.bridge {
            if b.middle.is_zero() || b.middle==NATIVE || self.reserves.contains(&b.middle) ||
               b.anchor.is_zero() || b.anchor==self.anchor || b.converters[0]==b.converters[1] ||
               b.converters.iter().any(|a|a.is_zero()) {return None;}
            path.extend([b.middle,b.anchor]);
        }
        path.push(self.reserves[1]);
        if i==1 {path.reverse();}
        Some(path)
    }
    pub fn native_asset(&self) -> Option<Address> {
        let side=self.reserves.iter().position(|a|*a==NATIVE)?;
        self.native.then_some(self.normalized[side])
    }
    fn asset(&self, token: Address) -> Address {
        if self.native_asset()==Some(token) {Address::ZERO} else {token}
    }
    pub fn value(&self, input: Address, amount: U256) -> U256 {
        if self.asset(input).is_zero(){amount}else{U256::ZERO}
    }
    pub(in crate::venues::bsc_dex) fn proof(self, order: &Order, minimum: U256, deadline: U256) -> Proof {
        Proof{terms:self, payer:order.payer, input:order.input, output:order.output, maximum:order.amount, minimum, deadline}
    }
}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct Proof {
    pub terms: Terms, pub payer: Address, pub input: Address, pub output: Address,
    pub maximum: U256, pub minimum: U256, pub deadline: U256,
}
fn unique<E: SolEvent>(logs: &[Log], address: Address) -> Option<E> {
    let mut events=logs.iter().filter(|l| l.address()==address && l.topic0()==Some(&E::SIGNATURE_HASH));
    let e=E::decode_log_data(&events.next()?.inner.data).ok()?;
    events.next().is_none().then_some(e)
}
impl Proof {
    pub fn data(&self) -> Bytes {
        let t=&self.terms;
        let path=t.path(self.input).unwrap_or_default();
        ILegacyWallet::executeCall{order:ILegacyWallet::WalletOrder{source:t.asset(self.input),target:t.asset(self.output),
            amount:self.maximum,minimum:self.minimum,deadline:self.deadline,path}}.abi_encode().into()
    }
    fn converter_events(&self,logs:&[Log],bridge:Bridge)->bool {
        use crate::venues::bsc_dex::pools::bancor_legacy::ILegacy;
        let Some(path)=self.terms.path(self.input) else{return false;};
        let mut owners=bridge.converters;
        if self.input==self.terms.normalized[1]{owners.reverse();}
        let mut amount=self.maximum;
        for (leg,owner) in path.windows(3).step_by(2).zip(owners){
            let Some(conv)=unique::<ILegacy::Conversion>(logs,owner) else{return false;};
            if (conv.sourceToken,conv.targetToken,conv.sourceAmount)!=(leg[0],leg[2],amount) ||
                ![self.terms.target,self.terms.network].contains(&conv.trader){return false;}
            amount=conv.targetAmount;
        }
        amount>=self.minimum
    }
    pub(super) async fn included(&self,ctx:&Ctx,hash:B256,r:&Receipt)->GatewayResult<()> {
        let t=&self.terms;
        signed_call::included(ctx,hash,r,self.payer,t.target,t.value(self.input,self.maximum),self.data()).await?;
        let block=r.block_hash.ok_or_else(||ctx.err(ErrorCode::OutcomeUnknown,"Bancor V2.1: квитанция без блока"))?;
        solidly_classic_factory::plain(ctx,t.target,t.module_hash,&serde_json::json!({"blockHash":block,"requireCanonical":true})).await?;
        if let Some(b)=t.bridge && !self.converter_events(&r.logs,b) {
            // Older converters may use another event ABI. Prove the owner at the parent and
            // absence of anchor events before our transaction, including its own receipt.
            use crate::venues::bsc_dex::node::{call::canonical,rpc};
            use serde_json::json;
            let unresolved=||ctx.err(ErrorCode::OutcomeUnknown,"Bancor V2.1: не подтверждён конвертер на позиции исходной операции");
            #[derive(Deserialize)]
            #[serde(rename_all="camelCase")]
            struct Header {hash:B256,parent_hash:B256,transactions:Vec<B256>}
            let (h,logs):(Header,Vec<Log>)=tokio::try_join!(
                rpc::call(ctx,"eth_getBlockByHash",json!([block,false])),
                rpc::call(ctx,"eth_getLogs",json!([{"blockHash":block,"address":[t.anchor,b.anchor]}]))
            )?;
            let i=r.transaction_index.ok_or_else(unresolved)?.to::<usize>();
            if h.hash!=block || h.transactions.get(i)!=Some(&hash) || logs.iter().any(|l|
                l.block_hash!=Some(block) || l.transaction_index.is_none_or(|n|n<=i as u64)) {return Err(unresolved());}
            let parent=json!({"blockHash":h.parent_hash,"requireCanonical":true});
            let (a,z)=tokio::try_join!(
                canonical(ctx,&parent,t.anchor,ILegacyNetwork::ownerCall{}),
                canonical(ctx,&parent,b.anchor,ILegacyNetwork::ownerCall{})
            )?;
            if [a,z]!=b.converters{return Err(unresolved());}
        }
        crate::venues::bsc_dex::send::canonical_receipt::verify(ctx,hash,r).await
    }
    pub fn measured(&self,logs:&[SimLog])->Option<(U256,U256)> {
        self.amounts(&signed_call::logs(logs)?,self.payer,self.input,self.output)
    }
    pub fn amounts(&self,logs:&[Log],payer:Address,input:Address,output:Address)->Option<(U256,U256)> {
        let t=&self.terms;
        let path=t.path(input)?;
        if (payer,input,output)!=(self.payer,self.input,self.output) || input==output || !t.normalized.contains(&output) ||
            self.minimum.is_zero() || self.maximum.is_zero() { return None; }
        let fill=unique::<ILegacyWallet::Paid>(logs,t.target)?;
        let mut events=logs.iter().filter(|l|l.address()==t.network && l.topic0()==Some(&ILegacyNetwork::Conversion::SIGNATURE_HASH));
        let mut amount=self.maximum;
        for leg in path.windows(3).step_by(2) {
            let log=events.next()?;
            let c=ILegacyNetwork::Conversion::decode_log_data(&log.inner.data).ok()?;
            if (c.source,c.anchor,c.target,c.trader)!=(leg[0],leg[1],leg[2],t.target) ||
                c.sourceAmount!=amount || c.targetAmount.is_zero() {return None;}
            amount=c.targetAmount;
        }
        if events.next().is_some(){return None;}
        if (fill.payer,fill.recipient,fill.pathHash,fill.source,fill.target)!=(payer,payer,keccak256(path.abi_encode()),t.asset(input),t.asset(output)) ||
            fill.spent!=self.maximum || amount<fill.received ||
            fill.received<self.minimum {return None;}
        let debits=|token| fill::moved_logs(logs,token,Some(payer),None);
        let credits=|token| fill::moved_logs(logs,token,None,Some(payer));
        if !fill.source.is_zero() && debits(input).checked_sub(credits(input))?!=fill.spent {return None;}
        if !fill.target.is_zero() && credits(output).checked_sub(debits(output))?!=fill.received {return None;}
        Some((fill.spent,fill.received))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn funded_original_wallet_fills_match_signed_terms_and_each_cash_leg() {
        let bytes=include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/bancor_v21_wallet.json.gz");
        let book:Value=serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
        let addr=|v:&Value|v.as_str().unwrap().parse::<Address>().unwrap();
        let units=|v:&Value|v.as_str().unwrap().parse::<U256>().unwrap();
        let wrapped=addr(&book["manifest"]["wrapped"]);
        let module=addr(&book["module"]);
        let index_bytes=include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/bancor_v21_ethereum.json.gz");
        let index:Value=serde_json::from_reader(flate2::read::GzDecoder::new(index_bytes.as_slice())).unwrap();
        let owner=|anchor|addr(&index["markets"].as_array().unwrap().iter().find(|m|addr(&m["anchor"])==anchor).unwrap()["converter"]);
        let mut checked=[0,0];
        for leg in book["controls"].as_array().unwrap().iter().filter(|r| r["mode"]!="v5") {
            let path:Vec<_>=leg["path"].as_array().unwrap().iter().map(addr).collect();
            let normalized=|t|if t==NATIVE {wrapped}else{t};
            let output=*path.last().unwrap();
            let mut sorted=path.clone();
            if normalized(path[0])>normalized(output){sorted.reverse();}
            let last=*sorted.last().unwrap();
            let bridge=(sorted.len()==5).then(||Bridge{middle:sorted[2],anchor:sorted[3],converters:[owner(sorted[1]),owner(sorted[3])]});
            let terms=Terms{target:module,network:addr(&book["manifest"]["network"]),normalized:[normalized(sorted[0]),normalized(last)],
                reserves:[sorted[0],last],anchor:sorted[1],bridge,native:leg["mode"]=="native_wallet",module_hash:book["manifest"]["runtimeKeccak"].as_str().unwrap().parse().unwrap()};
            let tx=&leg["tx"]; let data:Bytes=tx["data"].as_str().unwrap().parse().unwrap();
            let signed=ILegacyWallet::executeCall::abi_decode_validate(&data).unwrap();
            let logs:Vec<Log>=serde_json::from_value(leg["logs"].clone()).unwrap();
            let payer=unique::<ILegacyWallet::Paid>(&logs,module).unwrap().payer;
            let saved=Proof{terms,payer,input:normalized(path[0]),output:normalized(output),
                maximum:signed.order.amount,minimum:signed.order.minimum,deadline:signed.order.deadline};
            let persisted=serde_json::to_value(&saved).unwrap();
            assert_eq!(persisted["terms"].get("bridge").is_some(),bridge.is_some());
            let mut proof:Proof=serde_json::from_value(persisted).unwrap();
            assert_eq!(proof.data(),data);
            if let Some(b)=bridge {
                assert!(proof.converter_events(&logs,b));
                let mut wrong=b;
                wrong.converters.swap(0,1);
                assert!(!proof.converter_events(&logs,wrong));
                let mut swapped=logs.clone();
                let indices=swapped.iter().enumerate().filter_map(|(i,l)|(l.address()==terms.network && l.topic0()==Some(&ILegacyNetwork::Conversion::SIGNATURE_HASH)).then_some(i)).collect::<Vec<_>>();
                assert_eq!(indices.len(),2);
                swapped.swap(indices[0],indices[1]);
                assert!(proof.amounts(&swapped,payer,proof.input,proof.output).is_none());
                swapped.remove(indices[0]);
                assert!(proof.amounts(&swapped,payer,proof.input,proof.output).is_none());
            }
            assert_eq!(proof.amounts(&logs,payer,proof.input,proof.output),Some((units(&leg["spent"]),units(&leg["received"]))));
            assert_eq!(proof.amounts(&logs,Address::ZERO,proof.input,proof.output),None);
            proof.terms.anchor=Address::ZERO;
            assert_eq!(proof.amounts(&logs,payer,proof.input,proof.output),None);
            checked[usize::from(bridge.is_some())]+=1;
        }
        assert_eq!(checked,[16,12]);
    }
}
