//! Receipt evidence for an integral-share trade settled through the bound issuer wallet.
use super::{fill, signed_call};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, pools::solidly_classic_factory, quote::v2_wallet_sim::Order, send::track::Receipt},
};
use alloy::{rpc::types::Log, sol_types::{SolCall, SolEvent}};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};

alloy::sol! {
    interface IAktionariatWallet {
        struct ShareOrder {address broker; address source; address target; uint256 budget; uint256 minimum; uint256 deadline;}
        function execute(ShareOrder order) external returns(uint256);
        function registry() external view returns(address);
        function issuerHub() external view returns(address);
        function issuerMarkets(address market) external view returns(bool);
        event Paid(address indexed payer,address indexed recipient,address indexed broker,address source,address target,uint256 spent,uint256 received);
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Market {
    #[serde(default = "super::signed_call::legacy_ethereum_chain")]
    pub chain: u64,
    pub broker: Address,
    pub module: Address,
    pub runtime: B256,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub market: Market,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub budget: U256,
    pub minimum: U256,
    pub deadline: U256,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub v5: Option<signed_call::Erc20Call>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) issuer_steps: Vec<super::aktionariat_graph::IssuerStep>,
}
impl Market {
    pub(in crate::venues::bsc_dex) fn proof(self, order: &Order, minimum: U256, deadline: U256) -> Proof {
        Proof {market:self, payer:order.payer, input:order.input, output:order.output, budget:order.amount, minimum, deadline, v5:None, issuer_steps:Vec::new()}
    }
}
impl Proof {
    pub(in crate::venues::bsc_dex) fn data(&self) -> Bytes {
        IAktionariatWallet::executeCall { order: IAktionariatWallet::ShareOrder {
            broker:self.market.broker, source:self.input, target:self.output, budget:self.budget,
            minimum:self.minimum, deadline:self.deadline,
        }}.abi_encode().into()
    }

    pub(super) fn signed(&self) -> signed_call::Erc20Call {
        self.v5.unwrap_or_else(||signed_call::Erc20Call{chain:self.market.chain,payer:self.payer,module:self.market.module,calldata:keccak256(self.data())})
    }
    pub(super) async fn included(&self,ctx:&Ctx,hash:B256,nonce:u64,receipt:&Receipt)->GatewayResult<()> {
        self.signed().included(ctx,hash,nonce,receipt).await?;
        let h=receipt.block_hash.ok_or_else(||ctx.err(ErrorCode::OutcomeUnknown,"Aktionariat: в квитанции отсутствует блок"))?;
        let at=serde_json::json!({"blockHash":h,"requireCanonical":true});
        solidly_classic_factory::plain(ctx,self.market.module,self.market.runtime,&at).await?;
        let mut pinned = vec![(self.market.module, self.market.runtime)];
        for step in &self.issuer_steps {
            let identity = (step.market.module, step.market.runtime);
            if !pinned.contains(&identity) {
                solidly_classic_factory::plain(ctx, identity.0, identity.1, &at).await?;
                pinned.push(identity);
            }
        }
        if let Some(v)=self.v5 {
            let code=super::executor::code::runtime().map_err(|message|ctx.err(ErrorCode::OutcomeUnknown,message))?;
            solidly_classic_factory::plain(ctx,v.module,keccak256(code),&at).await?;
        }
        Ok(())
    }

    pub(in crate::venues::bsc_dex) fn measured(&self,logs:&[crate::venues::bsc_dex::check::simulate::SimLog])->Option<(U256,U256)> {
        self.amounts(&signed_call::logs(logs)?,self.payer,self.input,self.output)
    }

    pub(super) fn amounts(&self,logs:&[Log],wallet:Address,input:Address,output:Address)->Option<(U256,U256)> {
        if (wallet,input,output)!=(self.payer,self.input,self.output) || self.minimum.is_zero() ||
            input==output || wallet==self.market.module || wallet.is_zero() {return None;}
        if !self.issuer_steps.is_empty() {
            return super::aktionariat_graph::amounts(self, logs);
        }
        let mut events=logs.iter().filter(|l|l.address()==self.market.module &&
            l.topic0()==Some(&IAktionariatWallet::Paid::SIGNATURE_HASH));
        let paid=IAktionariatWallet::Paid::decode_log_data(&events.next()?.inner.data).ok()?;
        let payer=self.v5.map_or(wallet,|v|v.module);
        if events.next().is_some() || (paid.payer,paid.recipient,paid.broker,paid.source,paid.target)!=
                (payer,wallet,self.market.broker,input,output) ||
            paid.spent.is_zero() || paid.spent>self.budget || paid.received<self.minimum {return None;}
        let flow=|asset,from,to|fill::moved_logs(logs,asset,from,to);
        let spent=flow(input,Some(wallet),None).checked_sub(flow(input,None,Some(wallet)))?;
        let received=flow(output,None,Some(wallet)).checked_sub(flow(output,Some(wallet),None))?;
        if let Some(v)=self.v5 {
            let placed=flow(input,Some(wallet),Some(v.module));
            let returned=flow(input,Some(v.module),Some(wallet));
            // Pre-existing executor funds and unrelated transfers cannot be attributed to this broker.
            if placed!=self.budget || placed.checked_sub(returned)?!=spent ||
               flow(output,Some(self.market.module),Some(wallet))!=received {return None;}
        }
        ((spent,received)==(paid.spent,paid.received)).then_some((spent,received))
    }
}

/// The high 96 bits contain the signed debit bound on the first step. Subsequent
/// steps spend the preceding leg's output and retain the final route minimum.
pub(in crate::venues::bsc_dex) fn step_id(market:Address,cap:Option<U256>)->Option<B256>{
    if market.is_zero(){return None;}
    let mut id=market.into_word();
    if let Some(cap)=cap {
        let bytes=cap.to_be_bytes::<32>();
        if cap.is_zero() || bytes[..20].iter().any(|b|*b!=0){return None;}
        id.0[..12].copy_from_slice(&bytes[20..]);
    }
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn archived_signed_orders_recover_original_net_and_reject_a_different_broker(){
        let archive=include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/aktionariat_ethereum_protected.json.gz");
        let d:Value=serde_json::from_reader(flate2::read::GzDecoder::new(archive.as_slice())).unwrap();
        let address=|v:&Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let u=|v:&Value|v.as_str().unwrap().parse::<U256>().unwrap();
        let payer=address(&d["wallet"]);
        let module=address(&d["module"]);
        let exec=address(&d["executor"]);
        let mut checked=[0usize;2];
        for row in d["controls"].as_array().unwrap().iter().filter(|r|r.get("received").is_some()){
            let data:Bytes=row["request"]["data"].as_str().unwrap().parse().unwrap();
            let graph=row["mode"]=="v5";
            let budget=u(&row["budget"]);
            let source=address(&row["source"]);
            let target=address(&row["target"]);
            let broker=address(&row["broker"]);
            let market=Market{chain:crate::settings::evm_profiles::ETHEREUM_MAINNET,broker,module,runtime:d["manifest"]["runtimeKeccak"].as_str().unwrap().parse().unwrap()};
            let deadline=if graph{U256::ZERO}else{IAktionariatWallet::executeCall::abi_decode(&data).unwrap().order.deadline};
            let mut proof=Proof{market,payer,issuer_steps:Vec::new(),input:source,output:target,budget,minimum:u(&row["received"]),deadline,
                v5:graph.then(||signed_call::Erc20Call{chain:1,payer,module:exec,calldata:keccak256(&data)})};
            if !graph{assert_eq!(proof.data(),data);}
            let logs:Vec<crate::venues::bsc_dex::check::simulate::SimLog>=row["logs"].as_array().unwrap().iter().map(|l|{
                crate::venues::bsc_dex::check::simulate::SimLog{address:address(&l["address"]),
                    topics:serde_json::from_value(l["topics"].clone()).unwrap(),data:serde_json::from_value(l["data"].clone()).unwrap()}
            }).collect();
            assert_eq!(proof.measured(&logs),Some((u(&row["spent"]),u(&row["received"]))));
            proof.market.broker=payer;
            assert!(proof.measured(&logs).is_none());
            proof.market.broker=broker;proof.minimum+=U256::ONE;
            assert!(proof.measured(&logs).is_none());
            assert_eq!(row["refusal"].as_array().unwrap().len(),2);
            checked[usize::from(graph)]+=1;
        }
        assert_eq!(checked,[28,28]);
    }
}
