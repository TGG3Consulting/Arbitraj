//! Settlement proof for a factory BPool, its protected caller and the two wallet amounts.
use super::{fill, signed_call};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{check::simulate::SimLog, ctx::Ctx, pools::solidly_classic_factory,
        quote::v2_wallet_sim::Order, send::track::Receipt},
};
use alloy::{rpc::types::Log, sol_types::{SolCall, SolEvent}};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};

pub(in crate::venues::bsc_dex) use super::balancer_v1_abi::IBPoolWallet;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Terms {
    pub pool: Address,
    pub target: Address,
    pub wrapped: Address,
    pub native: bool,
    pub desired: Option<U256>,
    pub module_hash: B256,
    pub pool_hash: B256,
}
impl Terms {
    fn asset(self, token: Address) -> Address {
        if self.native && token == self.wrapped { Address::ZERO } else { token }
    }
    pub fn value(self, input: Address, maximum: U256) -> U256 {
        if self.asset(input).is_zero() { maximum } else { U256::ZERO }
    }
    pub(in crate::venues::bsc_dex) fn proof(self, o: &Order, minimum: U256, deadline: U256) -> Proof {
        Proof { terms:self, payer:o.payer, input:o.input, output:o.output, maximum:o.amount, minimum, deadline }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub terms: Terms,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub maximum: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Proof {
    pub fn data(&self) -> Bytes {
        let t = self.terms;
        IBPoolWallet::executeCall { order:IBPoolWallet::WalletOrder {
            pool:t.pool, assetIn:t.asset(self.input), assetOut:t.asset(self.output), maximum:self.maximum,
            minimum:t.desired.unwrap_or(self.minimum).max(self.minimum), maxPrice:U256::MAX,
            deadline:self.deadline, exactOutput:t.desired.is_some(),
        }}.abi_encode().into()
    }
    pub(super) async fn included(&self,ctx:&Ctx,hash:B256,r:&Receipt) -> GatewayResult<()> {
        let t=self.terms;
        signed_call::included(ctx,hash,r,self.payer,t.target,t.value(self.input,self.maximum),self.data()).await?;
        let block=r.block_hash.ok_or_else(||ctx.err(ErrorCode::OutcomeUnknown,"Balancer V1: нет блока квитанции"))?;
        let at=serde_json::json!({"blockHash":block,"requireCanonical":true});
        tokio::try_join!(solidly_classic_factory::plain(ctx,t.target,t.module_hash,&at),
                         solidly_classic_factory::plain(ctx,t.pool,t.pool_hash,&at))?;
        crate::venues::bsc_dex::send::canonical_receipt::verify(ctx,hash,r).await
    }
    pub fn measured(&self,logs:&[SimLog]) -> Option<(U256,U256)> {
        self.amounts(&signed_call::logs(logs)?,self.payer,self.input,self.output)
    }
    pub fn amounts(&self,logs:&[Log],payer:Address,input:Address,output:Address) -> Option<(U256,U256)> {
        let t=self.terms;
        if (payer,input,output)!=(self.payer,self.input,self.output) || input==output || self.minimum.is_zero() { return None; }
        let paid=single::<IBPoolWallet::Paid>(logs,t.target)?;
        let swap=single::<IBPoolWallet::LOG_SWAP>(logs,t.pool)?;
        if (paid.payer,paid.recipient,paid.pool,paid.assetIn,paid.assetOut)!=(payer,payer,t.pool,t.asset(input),t.asset(output)) ||
            paid.exactOutput!=t.desired.is_some() || paid.spent.is_zero() || paid.spent>self.maximum ||
            (t.desired.is_none() && paid.spent!=self.maximum) || paid.received<t.desired.unwrap_or(self.minimum).max(self.minimum) ||
            (swap.caller,swap.tokenIn,swap.tokenOut,swap.tokenAmountIn)!=(t.target,input,output,paid.spent) ||
            swap.tokenAmountOut<paid.received ||
            fill::moved_logs(logs,input,Some(t.target),Some(t.pool))!=paid.spent {return None;}
        if !paid.assetIn.is_zero() {
            let debits=fill::moved_logs(logs,input,Some(payer),None);
            let refunds=fill::moved_logs(logs,input,None,Some(payer));
            if debits.checked_sub(refunds)?!=paid.spent {return None;}
        }
        if !paid.assetOut.is_zero() {
            let credits=fill::moved_logs(logs,output,None,Some(payer));
            let debits=fill::moved_logs(logs,output,Some(payer),None);
            if credits.checked_sub(debits)?!=paid.received {return None;}
        }
        Some((paid.spent,paid.received))
    }
}
fn single<E:SolEvent>(logs:&[Log],contract:Address)->Option<E>{
    let mut hits=logs.iter().filter(|l|l.address()==contract && l.topic0()==Some(&E::SIGNATURE_HASH));
    let decoded=E::decode_log_data(&hits.next()?.inner.data).ok()?;
    hits.next().is_none().then_some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn canonical_bpool_events_pay_the_signed_wallet_with_the_recorded_refund() {
        let bytes=include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/balancer_v1_wallet.json.gz");
        let proof:Value=serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
        let addr=|v:&Value|v.as_str().unwrap().parse::<Address>().unwrap();
        let units=|v:&Value|v.as_str().unwrap().parse::<U256>().unwrap();
        let who=|role:&str|Address::from_slice(&alloy_primitives::keccak256(
            format!("Arbitraj Base 8453 isolated {role} 2026-11-26"))[12..]);
        let owner=who("owner");let executor=who("executor");
        let wrapped="0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2".parse::<Address>().unwrap();
        let mut checked=0;
        for row in proof["cases"].as_array().unwrap() {
            let pool=addr(&row["pool"]);let module=addr(&proof["module"]);
            let input=addr(&row["source"]);let output=addr(&row["target"]);
            let received=units(&row["received"]);let spent=units(&row["spent"]);
            let want=!row["exact_input"].as_bool().unwrap();
            let logs:Vec<SimLog>=serde_json::from_value(row["logs"].clone()).unwrap();
            let all=signed_call::logs(&logs).unwrap();
            let via=row["mode"]=="v5";
            let payment=single::<IBPoolWallet::Paid>(&all,module).unwrap();
            assert_eq!((payment.payer,payment.recipient,payment.pool,payment.spent,payment.received),
                (if via {executor}else{owner},owner,pool,spent,received));
            if via{
                assert_eq!(fill::moved_logs(&all,input,Some(owner),Some(executor)),spent);
                assert_eq!(fill::moved_logs(&all,output,None,Some(owner)),received);
            }else{
                let t=Terms{target:module,pool,wrapped,native:row["mode"]=="native_wallet",desired:want.then_some(received),
                    module_hash:proof["module_hash"].as_str().unwrap().parse().unwrap(),pool_hash:B256::ZERO};
                let p=Proof{terms:t,payer:owner,input,output,maximum:units(&row["maximum"]),minimum:received,deadline:U256::MAX};
                assert_eq!(p.measured(&logs),Some((spent,received)));
                if want {assert!(p.maximum>spent);}
                let mut clipped=p.clone();clipped.maximum=spent-U256::ONE;
                assert!(clipped.measured(&logs).is_none());
                let mut foreign=p.clone();foreign.terms.pool=module;
                assert!(foreign.measured(&logs).is_none());
                assert!(p.amounts(&all,executor,input,output).is_none());
            }
            checked+=1;
        }
        assert_eq!(checked,22);
    }
}
