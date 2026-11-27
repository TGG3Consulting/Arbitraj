//! Signed Bancor V3 amount mode, actual network fee class, vault funding, refund and beneficiary.
use super::{carbon_binding::Code,fill,signed_call};
use crate::{gateway::errors::{ErrorCode,GatewayResult},venues::bsc_dex::{
    check::simulate::SimLog,ctx::Ctx,pools::solidly_classic_factory,quote::v2_wallet_sim::Order,send::track::Receipt,
}};
use alloy::{rpc::types::Log,sol,sol_types::{SolCall,SolEvent}};
use alloy_primitives::{Address,Bytes,B256,U256};
use serde::{Deserialize,Serialize};

sol! { interface IBancorTrade {
    struct WalletOrder {address assetIn;address assetOut;uint256 form;uint256 maximum;uint256 minimum;uint256 deadline;bool exactOutput;}
    function execute(WalletOrder order) external payable returns(uint256 spent,uint256 received);
    function network() external view returns(address);
    function info() external view returns(address);
    function vault() external view returns(address);
    function wrapped() external view returns(address);
    function nativeToken() external view returns(address);
    function tradeBySourceAmount(address source,address target,uint256 amount,uint256 minimum,uint256 deadline,address recipient) external payable returns(uint256);
    function tradeByTargetAmount(address source,address target,uint256 quantity,uint256 maximum,uint256 deadline,address recipient) external payable returns(uint256);
    function tradeBySourceAmountArb(address source,address target,uint256 amount,uint256 minimum,uint256 deadline,address recipient) external payable returns(uint256);
    function tradeByTargetAmountArb(address source,address target,uint256 quantity,uint256 maximum,uint256 deadline,address recipient) external payable returns(uint256);
    event TokensTraded(bytes32 indexed contextId,address indexed sourceToken,address indexed targetToken,uint256 sourceAmount,uint256 targetAmount,uint256 bntAmount,uint256 targetFeeAmount,uint256 bntFeeAmount,address trader);
    event Paid(address indexed payer,address indexed recipient,address indexed venue,address source,address target,uint256 form,bool exactOutput,uint256 spent,uint256 received);
} }

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct Terms {
    pub network:Address, pub info:Address, pub vault:Address, pub wrapped:Address, pub eth:Address,
    pub source:Address, pub target:Address, pub contract:Address, pub network_code:Code, pub vault_code:Code,
    pub module_hash:Option<B256>, pub exemption:bool, pub native_wallet:bool, pub desired:Option<U256>,
}
impl Terms{
    pub fn form(&self)->u8{u8::from(self.source==self.eth)+2*u8::from(self.target==self.eth)+4*u8::from(self.exemption)}
    pub fn native(&self)->Option<Address>{self.native_wallet.then_some(self.wrapped)}
    pub fn value(&self,maximum:U256)->U256{if self.native_wallet && self.source==self.eth{maximum}else{U256::ZERO}}
    fn asset(&self,token:Address)->Address{if token!=self.eth{token}else if self.native_wallet{Address::ZERO}else{self.wrapped}}
    pub(in crate::venues::bsc_dex) fn proof(&self,o:&Order,minimum:U256,deadline:U256)->Proof{
        Proof{terms:self.clone(),payer:o.payer,input:o.input,output:o.output,maximum:o.amount,minimum,deadline}
    }
}

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct Proof{pub terms:Terms,pub payer:Address,pub input:Address,pub output:Address,pub maximum:U256,pub minimum:U256,pub deadline:U256}
fn event<E:SolEvent>(logs:&[Log],a:Address)->Option<E>{
    let mut matches=logs.iter().filter(|l|l.address()==a && l.topic0()==Some(&E::SIGNATURE_HASH));
    let e=E::decode_log_data(&matches.next()?.inner.data).ok()?;
    matches.next().is_none().then_some(e)
}
impl Proof{
    pub fn data(&self)->Bytes{
        let t=&self.terms;
        let (source,target,amount,minimum,deadline,recipient)=(t.source,t.target,self.maximum,self.minimum,self.deadline,self.payer);
        let quantity=t.desired.unwrap_or(minimum).max(minimum);
        if t.module_hash.is_some(){
            return IBancorTrade::executeCall{order:IBancorTrade::WalletOrder{assetIn:t.asset(source),assetOut:t.asset(target),
                form:U256::from(t.form()),maximum:amount,minimum:quantity,deadline,exactOutput:t.desired.is_some()}}.abi_encode().into();
        }
        match (t.exemption,t.desired.is_some()){
            (false,false)=>IBancorTrade::tradeBySourceAmountCall{source,target,amount,minimum,deadline,recipient}.abi_encode(),
            (true,false)=>IBancorTrade::tradeBySourceAmountArbCall{source,target,amount,minimum,deadline,recipient}.abi_encode(),
            (false,true)=>IBancorTrade::tradeByTargetAmountCall{source,target,quantity,maximum:amount,deadline,recipient}.abi_encode(),
            (true,true)=>IBancorTrade::tradeByTargetAmountArbCall{source,target,quantity,maximum:amount,deadline,recipient}.abi_encode(),
        }.into()
    }
    pub(super) async fn included(&self,ctx:&Ctx,hash:B256,r:&Receipt)->GatewayResult<()>{
        let t=&self.terms;
        signed_call::included(ctx,hash,r,self.payer,t.contract,t.value(self.maximum),self.data()).await?;
        let raw_native=(t.native_wallet && t.target==t.eth && t.module_hash.is_none()).then_some(self.payer);
        tokio::try_join!(t.network_code.at_transaction(ctx,t.network,hash,r,raw_native),t.vault_code.at_transaction(ctx,t.vault,hash,r,None))?;
        if let Some(pin)=t.module_hash{
            let block=r.block_hash.ok_or_else(||ctx.err(ErrorCode::OutcomeUnknown,"Bancor V3: отсутствует блок результата"))?;
            solidly_classic_factory::plain(ctx,t.contract,pin,&serde_json::json!({"blockHash":block,"requireCanonical":true})).await?;
        }
        crate::venues::bsc_dex::send::canonical_receipt::verify(ctx,hash,r).await
    }
    pub fn measured(&self,rows:&[SimLog])->Option<(U256,U256)>{self.amounts(&signed_call::logs(rows)?,self.payer,self.input,self.output)}
    pub fn amounts(&self,logs:&[Log],payer:Address,input:Address,output:Address)->Option<(U256,U256)>{
        let t=&self.terms;
        if (payer,input,output)!=(self.payer,self.input,self.output) || self.minimum.is_zero() || t.source==t.target || input==output{return None;}
        let trade=event::<IBancorTrade::TokensTraded>(logs,t.network)?;
        let actor=if t.module_hash.is_some(){t.contract}else{payer};
        if (trade.trader,trade.sourceToken,trade.targetToken)!=(actor,t.source,t.target)
            || trade.sourceAmount.is_zero() || trade.sourceAmount>self.maximum || (t.desired.is_none() && trade.sourceAmount!=self.maximum)
            || trade.targetAmount<self.minimum || (t.exemption && (!trade.targetFeeAmount.is_zero() || !trade.bntFeeAmount.is_zero())){return None;}
        if t.source!=t.eth && fill::moved_logs(logs,t.source,Some(actor),Some(t.vault))!=trade.sourceAmount{return None;}
        let net=|token,credit|{
            let give=fill::moved_logs(logs,token,Some(payer),None);let get=fill::moved_logs(logs,token,None,Some(payer));
            if credit{get.checked_sub(give)}else{give.checked_sub(get)}
        };
        let paid=if t.native_wallet && t.source==t.eth{trade.sourceAmount}else{net(input,false)?};
        let received=if t.native_wallet && t.target==t.eth{trade.targetAmount}else{net(output,true)?};
        if paid!=trade.sourceAmount || received<self.minimum || t.desired.is_some_and(|v|received<v){return None;}
        if t.module_hash.is_some(){
            let e=event::<IBancorTrade::Paid>(logs,t.contract)?;
            if (e.payer,e.recipient,e.venue,e.source,e.target,e.form,e.exactOutput,e.spent,e.received)!=(payer,payer,t.network,t.source,t.target,U256::from(t.form()),t.desired.is_some(),paid,received){return None;}
        }else if t.contract!=t.network || received!=trade.targetAmount{return None;}
        Some((paid,received))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn restore_original_and_protected_bancor_payments(){
        let unpack=|b:&[u8]|serde_json::from_reader::<_,Value>(flate2::read::GzDecoder::new(b)).unwrap();
        let native=unpack(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/bancor_v3_ethereum.json.gz"));
        let protected=unpack(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/bancor_v3_wallet.json.gz"));
        let addr=|s:&Value|s.as_str().unwrap().parse::<Address>().unwrap();
        let raw=|s:&Value|s.as_str().unwrap().parse::<U256>().unwrap();
        let roles=&native["roles"];
        let role=|name:&str|{
            let v=&roles[name];
            Code{shell:v["keccak256"].as_str().unwrap().parse().unwrap(),implementation:addr(&v["implementation"]["address"]),
                runtime:v["implementation"]["keccak256"].as_str().unwrap().parse().unwrap()}
        };
        let network=addr(&roles["BancorNetwork"]["address"]);let vault=addr(&roles["MasterVault"]["address"]);
        let module=addr(&protected["module"]);let eth=Address::repeat_byte(0xee);
        let wrapped="0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2".parse::<Address>().unwrap();
        let who=|suffix:&str|Address::from_slice(&alloy_primitives::keccak256(format!("Arbitraj Base 8453 isolated {suffix} 2026-11-26"))[12..]);
        let owner=who("owner");let executor=who("executor");
        let mut count=0;
        for (is_module,fixture) in [(false,&native),(true,&protected)]{
            for c in fixture["cases"].as_array().unwrap().iter().filter(|c|c["spent"].is_string()){
                let target=addr(&c["target"]);let source=addr(&c["source"]);
                let paid=raw(&c["spent"]);let received=raw(&c["received"]);
                let input=if source==eth{wrapped}else{source};let output=if target==eth{wrapped}else{target};
                let rows:Vec<SimLog>=serde_json::from_value(c["logs"].clone()).unwrap();
                let logs=signed_call::logs(&rows).unwrap();
                let via=c["mode"]=="v5";
                let mode=!c["exact_input"].as_bool().unwrap();
                let t=Terms{network,vault,wrapped,eth,source,target,network_code:role("BancorNetwork"),vault_code:role("MasterVault"),
                    info:addr(&roles["BancorNetworkInfo"]["address"]),contract:if is_module{module}else{network},
                    module_hash:is_module.then(||protected["module_hash"].as_str().unwrap().parse().unwrap()),exemption:false,
                    native_wallet:if is_module{c["mode"]=="native_wallet"}else{source==eth || target==eth},desired:mode.then_some(received)};
                if via{
                    let e=event::<IBancorTrade::Paid>(&logs,module).unwrap();
                    assert_eq!((e.payer,e.recipient,e.source,e.target,e.venue,e.spent,e.received,e.exactOutput),(executor,owner,source,target,network,paid,received,false));
                    assert_eq!(fill::moved_logs(&logs,input,Some(owner),Some(executor)),paid);
                    assert_eq!(fill::moved_logs(&logs,output,None,Some(owner)),received);
                }else{
                    let maximum=if is_module{raw(&c["maximum"])}else{paid};
                    let proof=Proof{terms:t,payer:owner,input,output,maximum,minimum:received,deadline:U256::MAX};
                    assert_eq!(proof.measured(&rows),Some((paid,received)));
                    assert!(proof.amounts(&logs,executor,input,output).is_none());
                    let mut clipped=proof.clone();clipped.maximum=paid-U256::ONE;
                    assert!(clipped.measured(&rows).is_none());
                    let mut no_source=rows.clone();no_source.retain(|l|l.address!=network);
                    assert!(proof.measured(&no_source).is_none());
                    let other=Proof{terms:Terms{exemption:true,..proof.terms.clone()},..proof};
                    if event::<IBancorTrade::TokensTraded>(&logs,network).unwrap().targetFeeAmount!=U256::ZERO{
                        assert!(other.measured(&rows).is_none());
                    }
                }
                count+=1;
            }
        }
        assert_eq!(count,64);
    }
}
