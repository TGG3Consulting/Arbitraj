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

#[derive(Debug,Clone,Copy,Serialize,Deserialize)]
pub struct Terms {
    pub target: Address,
    pub network: Address,
    pub normalized: [Address;2],
    pub reserves: [Address;2],
    pub anchor: Address,
    pub native: bool,
    pub module_hash: B256,
}
impl Terms {
    pub fn path(&self, input: Address) -> Option<[Address;3]> {
        let i=self.normalized.iter().position(|t| *t==input)?;
        Some([self.reserves[i], self.anchor, self.reserves[1-i]])
    }
    pub fn native_asset(&self) -> Option<Address> {
        let side=self.reserves.iter().position(|a|*a==NATIVE)?;
        self.native.then_some(self.normalized[side])
    }
    fn asset(&self, token: Address) -> Address {
        if self.native && self.path(token).is_some_and(|p| p[0]==NATIVE) {Address::ZERO} else {token}
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
        let path=t.path(self.input).map(Vec::from).unwrap_or_default();
        ILegacyWallet::executeCall{order:ILegacyWallet::WalletOrder{source:t.asset(self.input),target:t.asset(self.output),
            amount:self.maximum,minimum:self.minimum,deadline:self.deadline,path}}.abi_encode().into()
    }
    pub(super) async fn included(&self,ctx:&Ctx,hash:B256,r:&Receipt)->GatewayResult<()> {
        let t=&self.terms;
        signed_call::included(ctx,hash,r,self.payer,t.target,t.value(self.input,self.maximum),self.data()).await?;
        let block=r.block_hash.ok_or_else(||ctx.err(ErrorCode::OutcomeUnknown,"Bancor V2.1: квитанция без блока"))?;
        solidly_classic_factory::plain(ctx,t.target,t.module_hash,&serde_json::json!({"blockHash":block,"requireCanonical":true})).await?;
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
        let network=unique::<ILegacyNetwork::Conversion>(logs,t.network)?;
        if (fill.payer,fill.recipient,fill.pathHash,fill.source,fill.target)!=(payer,payer,keccak256(path.to_vec().abi_encode()),t.asset(input),t.asset(output)) ||
            (network.anchor,network.source,network.target,network.trader)!=(path[1],path[0],path[2],t.target) ||
            network.sourceAmount!=self.maximum || fill.spent!=self.maximum || network.targetAmount<fill.received ||
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
        let mut checked=0;
        for leg in book["controls"].as_array().unwrap().iter().filter(|r| r["mode"]!="v5" && r["path"].as_array().unwrap().len()==3) {
            let path:Vec<_>=leg["path"].as_array().unwrap().iter().map(addr).collect();
            let normalized=|t|if t==NATIVE {wrapped}else{t};
            let terms=Terms{target:module,network:addr(&book["manifest"]["network"]),normalized:[normalized(path[0]),normalized(path[2])],
                reserves:[path[0],path[2]],anchor:path[1],native:leg["mode"]=="native_wallet",module_hash:book["manifest"]["runtimeKeccak"].as_str().unwrap().parse().unwrap()};
            let tx=&leg["tx"]; let data:Bytes=tx["data"].as_str().unwrap().parse().unwrap();
            let signed=ILegacyWallet::executeCall::abi_decode_validate(&data).unwrap();
            let logs:Vec<Log>=serde_json::from_value(leg["logs"].clone()).unwrap();
            let payer=unique::<ILegacyWallet::Paid>(&logs,module).unwrap().payer;
            let mut proof=Proof{terms,payer,input:terms.normalized[0],output:terms.normalized[1],
                maximum:signed.order.amount,minimum:signed.order.minimum,deadline:signed.order.deadline};
            assert_eq!(proof.data(),data);
            assert_eq!(proof.amounts(&logs,payer,proof.input,proof.output),Some((units(&leg["spent"]),units(&leg["received"]))));
            assert_eq!(proof.amounts(&logs,Address::ZERO,proof.input,proof.output),None);
            proof.terms.anchor=Address::ZERO;
            assert_eq!(proof.amounts(&logs,payer,proof.input,proof.output),None);
            checked+=1;
        }
        assert_eq!(checked,16);
    }
}
