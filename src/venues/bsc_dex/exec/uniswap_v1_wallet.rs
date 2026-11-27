//! Reconstruct the signed V1 call and identify both physical exchanges and the wallet payout.
use super::{fill, signed_call};
use crate::{gateway::errors::{ErrorCode, GatewayResult}, venues::bsc_dex::{
    check::simulate::SimLog, ctx::Ctx, pools::solidly_classic_factory::plain, quote::v2_wallet_sim::Order,
    send::track::Receipt,
}};
use alloy::{rpc::types::Log, sol, sol_types::{SolCall, SolEvent}};
use alloy_primitives::{Address, Bytes, B256, U256};
use serde::{Deserialize, Serialize};

sol! {
    interface IV1Wallet {
        function factory() external view returns(address);
        function wrapped() external view returns(address);
        function executeNative(address pool,address token,bool nativeIn,uint256 amount,uint256 minimum,uint256 deadline) external payable returns(uint256);
        function execute(address pool,address tokenIn,address tokenOut,uint256 amount,uint256 minimum,uint256 deadline) external returns(uint256);
        function executePair(address pool,address targetPool,address tokenIn,address tokenOut,uint256 amount,uint256 minimum,uint256 deadline) external returns(uint256);
        function ethToTokenSwapInput(uint256 minimum,uint256 deadline) external payable returns(uint256);
        function tokenToEthSwapInput(uint256 amount,uint256 minimum,uint256 deadline) external returns(uint256);
        function tokenToTokenSwapInput(uint256 amount,uint256 minimum,uint256 bridgeMinimum,uint256 deadline,address tokenOut) external returns(uint256);
        event Filled(address indexed payer,address indexed exchange,address indexed recipient,address assetIn,address assetOut,uint256 paid,uint256 received);
        event TokenPurchase(address indexed buyer,uint256 indexed ethSold,uint256 indexed tokensBought);
        event EthPurchase(address indexed buyer,uint256 indexed tokensSold,uint256 indexed ethBought);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Terms {
    pub target: Address,
    pub wrapped: Address,
    /// Absence of the input or output exchange denotes physical ETH at that end.
    pub input_pool: Option<Address>,
    pub output_pool: Option<Address>,
    pub exchange_hash: B256,
    pub module_hash: Option<B256>,
    #[serde(default)]
    pub physical_module:bool,
}
impl Terms {
    fn pool(&self) -> Option<Address> { self.input_pool.or(self.output_pool) }
    pub fn native(&self) -> Option<Address> {
        ((self.module_hash.is_none() || self.physical_module) && (self.input_pool.is_none() || self.output_pool.is_none())).then_some(self.wrapped)
    }
    pub fn value(&self, amount: U256) -> U256 {
        if self.native().is_some() && self.input_pool.is_none() { amount } else { U256::ZERO }
    }
    pub(in crate::venues::bsc_dex) fn proof(&self,o:&Order,minimum:U256,deadline:U256)->Proof{
        Proof{terms:self.clone(),payer:o.payer,input:o.input,output:o.output,amount:o.amount,minimum,deadline}
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub terms: Terms,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub deadline: U256,
}
fn event<E:SolEvent>(logs:&[Log],address:Address)->Option<E>{
    let mut matching=logs.iter().filter(|l|l.address()==address && l.topic0()==Some(&E::SIGNATURE_HASH));
    let e=E::decode_log_data(&matching.next()?.inner.data).ok()?;
    matching.next().is_none().then_some(e)
}
impl Proof {
    pub fn data(&self)->Bytes {
        let t=&self.terms;
        let (minimum,amount,deadline,token_out)=(self.minimum,self.amount,self.deadline,self.output);
        if t.physical_module && let Some(pool)=t.pool() {
            let native_in=t.input_pool.is_none();
            return IV1Wallet::executeNativeCall{pool,token:if native_in{token_out}else{self.input},nativeIn:native_in,amount,minimum,deadline}.abi_encode().into();
        }
        match (t.module_hash,t.input_pool,t.output_pool) {
            (Some(_),Some(pool),Some(target_pool))=>IV1Wallet::executePairCall{pool,targetPool:target_pool,tokenIn:self.input,tokenOut:token_out,amount,minimum,deadline}.abi_encode(),
            (Some(_),Some(pool),None)|(Some(_),None,Some(pool))=>IV1Wallet::executeCall{pool,tokenIn:self.input,tokenOut:token_out,amount,minimum,deadline}.abi_encode(),
            (None,None,Some(_))=>IV1Wallet::ethToTokenSwapInputCall{minimum,deadline}.abi_encode(),
            (None,Some(_),None)=>IV1Wallet::tokenToEthSwapInputCall{amount,minimum,deadline}.abi_encode(),
            (None,Some(_),Some(_))=>IV1Wallet::tokenToTokenSwapInputCall{amount,minimum,bridgeMinimum:U256::ONE,deadline,tokenOut:token_out}.abi_encode(),
            (_,None,None)=>Vec::new(),
        }.into()
    }
    pub(super) async fn included(&self,ctx:&Ctx,h:B256,r:&Receipt)->GatewayResult<()>{
        let t=&self.terms;
        signed_call::included(ctx,h,r,self.payer,t.target,t.value(self.amount),self.data()).await?;
        let hash=r.block_hash.ok_or_else(||ctx.err(ErrorCode::OutcomeUnknown,"Uniswap V1: хеш блока отсутствует"))?;
        let at=serde_json::json!({"blockHash": hash,"requireCanonical":true});
        for pool in [t.input_pool,t.output_pool].into_iter().flatten(){plain(ctx,pool,t.exchange_hash,&at).await?;}
        if let Some(pin)=t.module_hash{plain(ctx,t.target,pin,&at).await?;}
        crate::venues::bsc_dex::send::canonical_receipt::verify(ctx,h,r).await
    }
    pub fn measured(&self,rows:&[SimLog])->Option<(U256,U256)>{
        self.amounts(&signed_call::logs(rows)?,self.payer,self.input,self.output)
    }
    pub fn amounts(&self,logs:&[Log],owner:Address,input:Address,output:Address)->Option<(U256,U256)>{
        let t=&self.terms;
        if (owner,input,output)!=(self.payer,self.input,self.output) || input==output || self.amount.is_zero() || self.minimum.is_zero()
            || t.input_pool==t.output_pool {return None;}
        let actor=if t.module_hash.is_some(){t.target}else{owner};
        let bridge=if let Some(pool)=t.input_pool{
            let s=event::<IV1Wallet::EthPurchase>(logs,pool)?;
            if s.buyer!=actor || s.tokensSold!=self.amount{return None;}
            s.ethBought
        }else{self.amount};
        let nominal=if let Some(pool)=t.output_pool{
            let b=event::<IV1Wallet::TokenPurchase>(logs,pool)?;
            if b.buyer!=t.input_pool.unwrap_or(actor) || b.ethSold!=bridge{return None;}
            b.tokensBought
        }else{bridge};
        let net=|asset,credit|{
            let out=fill::moved_logs(logs,asset,Some(owner),None);
            let inc=fill::moved_logs(logs,asset,None,Some(owner));
            if credit{inc.checked_sub(out)}else{out.checked_sub(inc)}
        };
        let paid=if t.native()==Some(input){self.amount}else{net(input,false)?};
        let received=if t.native()==Some(output){bridge}else{net(output,true)?};
        if paid!=self.amount || bridge.is_zero() || received<self.minimum{return None;}
        if t.module_hash.is_some(){
            let e=event::<IV1Wallet::Filled>(logs,t.target)?;
            if (e.payer,e.exchange,e.recipient,e.assetIn,e.assetOut,e.paid,e.received)!=(owner,t.pool()?,owner,if t.native()==Some(input){Address::ZERO}else{input},if t.native()==Some(output){Address::ZERO}else{output},paid,received){return None;}
        }else if Some(t.target)!=t.pool() || nominal!=received{return None;}
        Some((paid,received))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn wallet_and_executor_receipts_match_all_paid_archive_cases(){
        let raw=include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/uniswap_v1_ethereum.json.gz");
        let fixture:Value=serde_json::from_reader(flate2::read::GzDecoder::new(raw.as_slice())).unwrap();
        let addr=|v:&Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let module=addr(&fixture["module"]);
        let isolated=|role:&str| Address::from_slice(&alloy_primitives::keccak256(format!("Arbitraj Base 8453 isolated {role} 2026-11-26"))[12..]);
        let owner=isolated("owner");
        let executor=isolated("executor");
        let wrapped="0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2".parse::<Address>().unwrap();
        let mut cases=0;
        for c in fixture["cases"].as_array().unwrap().iter().filter(|c|c["spent"].is_string()){
            let mode=c["mode"].as_str().unwrap();
            let via=mode.starts_with("v5");
            let physical_module=mode=="native_wallet";
            let protected=via || physical_module || mode.starts_with("wallet");
            let src=addr(&c["source"]);let dst=addr(&c["target"]);let pool=addr(&c["pool"]);
            let input=if src.is_zero(){wrapped}else{src};let output=if dst.is_zero(){wrapped}else{dst};
            let (first,last)=if c["target_pool"].is_string(){(Some(pool),Some(addr(&c["target_pool"])))}else if input==wrapped{(None,Some(pool))}else{(Some(pool),None)};
            let pin=fixture["markets"].as_object().unwrap().values().next().unwrap()["runtime"].as_str().unwrap().parse().unwrap();
            let t=Terms{target:if protected{module}else{pool},wrapped,physical_module,input_pool:first,output_pool:last,exchange_hash:pin,
                module_hash:protected.then(||fixture["module_hash"].as_str().unwrap().parse().unwrap())};
            let amount=c["spent"].as_str().unwrap().parse().unwrap();
            let received=c["received"].as_str().unwrap().parse().unwrap();
            let sim:Vec<SimLog>=serde_json::from_value(c["logs"].clone()).unwrap();
            let logs=signed_call::logs(&sim).unwrap();
            if via {
                let event=event::<IV1Wallet::Filled>(&logs,module).unwrap();
                assert_eq!((event.payer,event.recipient,event.exchange,event.assetIn,event.assetOut,event.paid,event.received),
                           (executor,owner,pool,input,output,amount,received));
                assert_eq!(fill::moved_logs(&logs,input,Some(owner),Some(executor)),amount);
                assert_eq!(fill::moved_logs(&logs,output,None,Some(owner)),received);
            }else{
                let proof=Proof{terms:t,payer:owner,input,output,amount,minimum:received,deadline:U256::MAX};
                assert_eq!(proof.measured(&sim),Some((amount,received)), "{mode}");
                assert!(proof.amounts(&logs,executor,input,output).is_none());
                let mut raised=proof.clone();raised.minimum+=U256::ONE;
                assert!(raised.measured(&sim).is_none());
                let mut missing=sim.clone();missing.retain(|l|l.address!=pool);
                assert!(proof.measured(&missing).is_none());
            }
            cases+=1;
        }
        assert!(cases>=36);
    }
}
