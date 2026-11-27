//! Денежный результат AlgebraRouterWallet берётся из изменений балансов, измеренных контрактом.
use super::registered_v3_wallet::{Proof,RouterMarket};
use super::super::{ctx::Ctx,node::rpc,send::track::Receipt};
use crate::gateway::errors::{ErrorCode,GatewayResult};
use alloy::{rpc::types::Log,sol_types::{SolCall,SolEvent}};
use alloy_primitives::{Address,B256,Bytes,U256,keccak256};
use serde_json::json;

alloy::sol! {
    interface Wallet {
        function execute(address pool,address input,address output,address deployer,uint256 amount,uint256 minimum,uint256 deadline)
            external returns(uint256 spent,uint256 received);
        event Settled(address indexed payer,address indexed pool,address indexed input,address output,uint256 spent,uint256 received);
    }
}

pub(super) fn order(pool:Address,input:Address,output:Address,deployer:Address,amount:U256,minimum:U256,deadline:U256)->Bytes {
    Wallet::executeCall{pool,input,output,deployer,amount,minimum,deadline}.abi_encode().into()
}

pub(super) async fn included(ctx:&Ctx,target:Address,expected:B256,r:&Receipt)->GatewayResult<()> {
    let bad=||ctx.err(ErrorCode::OutcomeUnknown,"Не подтверждён runtime исходного модуля Algebra на блоке включения");
    let block=r.block_hash.ok_or_else(bad)?;
    let code:Bytes=rpc::call(ctx,"eth_getCode",json!([target,{"blockHash":block,"requireCanonical":true}])).await.map_err(|_|bad())?;
    if code.is_empty() || keccak256(code)!=expected {return Err(bad());}
    Ok(())
}

pub(super) fn amounts<M:RouterMarket>(p:&Proof<M>,router:Address,logs:&[Log])->Option<(U256,U256)> {
    let mut fills=logs.iter().filter(|l|l.address()==p.target && l.topic0()==Some(&Wallet::Settled::SIGNATURE_HASH));
    let fill=Wallet::Settled::decode_log_data(&fills.next()?.inner.data).ok()?;
    if fills.next().is_some() || (fill.payer,fill.pool,fill.input,fill.output)!=(p.payer,p.market.emitter(),p.input,p.output)
        || fill.spent.is_zero() || fill.spent>p.budget || fill.received<p.minimum {return None;}
    let (event,bytes)=p.market.event();
    let topic=keccak256(event);
    let mut swaps=logs.iter().filter(|l|l.address()==fill.pool && l.topic0()==Some(&topic));
    let swap=swaps.next()?;
    if swaps.next().is_some() || swap.topics()!=[topic,router.into_word(),p.payer.into_word()] || swap.data().data.len()!=bytes {return None;}
    let data=&swap.data().data;
    let amount=|i:usize|U256::from_be_slice(&data[i*32..(i+1)*32]);
    let input=usize::from(p.input>p.output);
    let raw_in=amount(input);
    let raw_out=amount(1-input);
    if raw_in.is_zero() || raw_in>p.budget || !raw_out.bit(255) {return None;}
    Some((fill.spent,fill.received))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::bsc_dex::{check::simulate::SimLog,exec::registered_v3_wallet::Market};
    use alloy_primitives::aliases::U24;
    use flate2::read::GzDecoder;
    use serde_json::Value;

    #[test]
    fn protected_order_restores_all_net_payments_including_both_rounded_sales() {
        let archive:Value=serde_json::from_reader(GzDecoder::new(include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/theorem_ethereum.json.gz").as_slice())).unwrap();
        let w=&archive["wallets"];
        let a=|v:&Value|v.as_str().unwrap().parse::<Address>().unwrap();
        let n=|v:&Value|v.as_str().unwrap().parse::<U256>().unwrap();
        let router=a(&archive["directory"]["roles"]["router"]["address"]);
        let mut matched=0;
        let mut rounded=0;
        for r in w["cases"].as_array().unwrap().iter().filter(|r|r["wire"]=="module") {
            for side in ["buy","sell"] {
                let leg=&r[side];
                let Some(tx)=leg.get("transaction") else {continue};
                let data:Bytes=tx["data"].as_str().unwrap().parse().unwrap();
                let call=Wallet::executeCall::abi_decode(&data).unwrap();
                let market=Market{pool:call.pool,fee:U24::ZERO,algebra_deployer:Some(call.deployer),
                    algebra_short_event:true,algebra_wallet:Some((router,w["module_hash"].as_str().unwrap().parse().unwrap())),
                    fee_event:false,wallet_module:true,maverick:false,maverick_chain:None,slipstream:None,router02:None};
                let proof=Proof{market,target:a(&tx["to"]),payer:a(&tx["from"]),input:call.input,output:call.output,
                    budget:call.amount,minimum:call.minimum,deadline:call.deadline};
                let mut restored:Proof=serde_json::from_value(serde_json::to_value(proof).unwrap()).unwrap();
                let logs:Vec<SimLog>=serde_json::from_value(leg["logs"].clone()).unwrap();
                let spent=n(&leg["spent"]);
                assert_eq!(restored.data(),data);
                assert_eq!(restored.measured(&logs),Some((spent,n(&leg["received"]))));
                if spent!=call.amount {assert_eq!(spent+U256::ONE,call.amount);rounded+=1;}
                restored.minimum+=U256::ONE;
                assert!(restored.measured(&logs).is_none());
                matched+=1;
            }
        }
        assert_eq!((matched,rounded),(28,2));
    }
}
