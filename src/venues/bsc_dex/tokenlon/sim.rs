//! Одна полная выплата без подмены балансов. ETH у PMMV5 передаётся через msg.value.
use super::receipt::Proof;
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{check::simulate::CallResult, ctx::Ctx, exec::signed_call, node::rpc, pools::solidly_classic_factory as binding},
};
use alloy_primitives::{Address, B256, Bytes, U64, U256, keccak256};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Header {hash:B256, number:U64}
#[derive(Deserialize)]
#[serde(rename_all="camelCase")]
struct Block {parent_hash:B256, number:U64, timestamp:U64, calls:Vec<CallResult>}

fn balance(who:Address,asset:Address,multicall:Address,weth:Address)->Value {
    let (to,name)=if asset==weth {(multicall,"getEthBalance(address)")}else{(asset,"balanceOf(address)")};
    let mut data=keccak256(name.as_bytes())[..4].to_vec();
    data.extend_from_slice(&[0;12]);
    data.extend_from_slice(who.as_slice());
    json!({"from":who,"to":to,"data":Bytes::from(data)})
}
fn read(who:Address,to:Address,signature:&str,args:&[Address])->Value{
    let mut data=keccak256(signature.as_bytes())[..4].to_vec();
    for a in args{data.extend_from_slice(&[0;12]);data.extend_from_slice(a.as_slice());}
    json!({"from":who,"to":to,"data":Bytes::from(data)})
}

/// Только единственное полное слово ABI успешного ответа. Пустая allowance не означает разрешения.
fn word(c:&CallResult)->Option<U256> {
    (c.ok() && c.return_data.len()==32).then(||U256::from_be_slice(&c.return_data))
}

fn matches_role(c:&CallResult,address:Address)->bool {
    word(c)==Some(U256::from_be_slice(address.as_slice()))
}

pub(super) fn value(p:&Proof)->U256 {
    if p.terms.input==p.terms.roles.weth.address {p.terms.spent} else {U256::ZERO}
}

pub(super) async fn run(ctx:&Ctx,proof:&Proof,data:&Bytes)->GatewayResult<(u64,u64)> {
    let reject=||ctx.err(ErrorCode::QuoteMismatch,"Tokenlon: симуляция не подтвердила точные расходы, зачисление и канонический исходный блок");
    let (head,chain):(Header,U64)=tokio::try_join!(
        rpc::call(ctx,"eth_getBlockByNumber",json!(["latest",false])),
        rpc::call(ctx,"eth_chainId",json!([])))?;
    let roles=&proof.terms.roles;
    if chain.to::<u64>()!=roles.chain || ctx.chain_id()!=Some(roles.chain) {return Err(reject());}
    let tag=json!({"blockHash":head.hash,"requireCanonical":true});
    let t=&proof.terms;
    let actor:Bytes=rpc::call(ctx,"eth_getCode",json!([t.payer,&tag])).await?;
    if !actor.is_empty() {return Err(reject());}
    tokio::try_join!(
        binding::bind(ctx,roles.user_proxy.address,roles.user_proxy.runtime,Some((roles.proxy_impl.address,roles.proxy_impl.runtime)),&tag),
        binding::plain(ctx,roles.pmm.address,roles.pmm.runtime,&tag),
        binding::plain(ctx,roles.zero_exchange.address,roles.zero_exchange.runtime,&tag),
        super::assets::exact_transfers(ctx,t.input,t.output,&tag))?;
    let mc=ctx.text("dex.multicall3")?.parse::<Address>().map_err(|_|reject())?;
    let input=balance(t.payer,t.input,mc,roles.weth.address);
    let output=balance(t.recipient,t.output,mc,roles.weth.address);
    let action=json!({"from":t.payer,"to":roles.user_proxy.address,"data":data,"value":value(proof)});
    let mut calls=vec![read(t.payer,roles.pmm.address,"userProxy()",&[]),
        read(t.payer,roles.pmm.address,"zeroExchange()",&[])];
    if t.input!=roles.weth.address {
        tokio::try_join!(
            binding::plain(ctx,roles.spender.address,roles.spender.runtime,&tag),
            binding::plain(ctx,roles.allowance_target.address,roles.allowance_target.runtime,&tag))?;
        calls.push(read(t.payer,roles.pmm.address,"spender()",&[]));
        calls.push(read(t.payer,roles.spender.address,"allowanceTarget()",&[]));
        calls.push(read(t.payer,t.input,"allowance(address,address)",&[t.payer,roles.allowance_target.address]));
    }
    calls.extend([input.clone(),output.clone(),action,input,output]);
    // В запросе находится личная подпись fill. Ошибка транспорта не должна содержать исходные params.
    let blocks:Vec<Block>=super::private_node::simulate(ctx,json!([{"blockStateCalls":[{
        "calls":calls}]},&tag])).await?;
    let [b]=blocks.as_slice() else{return Err(reject())};
    let [proxy,exchange,remaining @ ..]=b.calls.as_slice() else{return Err(reject())};
    if !matches_role(proxy,roles.user_proxy.address)
        || !matches_role(exchange,roles.zero_exchange.address) {return Err(reject());}
    let payments=if t.input!=roles.weth.address {
        let [spender,target,limit,rest @ ..]=remaining else{return Err(reject())};
        if !matches_role(spender,roles.spender.address) ||
            !matches_role(target,roles.allowance_target.address) {return Err(reject());}
        if word(limit).ok_or_else(reject)?<t.spent {
            return Err(ctx.err(ErrorCode::InsufficientPermissions,"Tokenlon: разрешения пользователя для подтверждённого AllowanceTarget недостаточно"));
        }
        rest
    } else {remaining};
    let [i0,o0,swap,i1,o1]=payments else{return Err(reject())};
    if word(i0).is_some_and(|n|n<t.spent) {return Err(ctx.err(ErrorCode::InsufficientBalance,"Tokenlon: недостаточно входного актива кошелька"));}
    let flows=word(i0).zip(word(i1)).and_then(|(a,b)|a.checked_sub(b))
        .zip(word(o1).zip(word(o0)).and_then(|(a,b)|a.checked_sub(b)));
    let events=signed_call::logs(&swap.logs).and_then(|logs|proof.amounts(&logs));
    if b.parent_hash!=head.hash || b.number.to::<u64>()!=head.number.to::<u64>().saturating_add(1)
        || b.timestamp.to::<u64>()>=proof.expiry || !swap.ok()
        || flows!=Some((t.spent,proof.net)) || events!=flows {
        return Err(reject());
    }
    Ok((head.number.to::<u64>(),swap.gas_used.to::<u64>()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_pmm_getters_reject_wrong_role_and_malformed_abi_words() {
        let record:Value=serde_json::from_str(include_str!("../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_roles.json")).unwrap();
        let rows:Vec<String>=serde_json::from_value(record["roles"].clone()).unwrap();
        let roles=super::super::roles::FrozenRoles::from_rows(1,&rows).unwrap();
        for (getter,role) in [("userProxy()",roles.user_proxy),("zeroExchange()",roles.zero_exchange),("spender()",roles.spender)] {
            let raw=&record["reads"][format!("{}.{}",roles.pmm.address.to_string().to_lowercase(),getter)];
            let mut result:CallResult=serde_json::from_value(json!({"status":"0x1","returnData":raw,"gasUsed":"0x0"})).unwrap();
            assert!(matches_role(&result,role.address));
            assert!(!matches_role(&result,roles.pmm.address));
            result.status=U64::ZERO;
            assert!(word(&result).is_none());
            result.status=U64::from(1);
            result.return_data=Bytes::copy_from_slice(&result.return_data[1..]);
            assert!(word(&result).is_none());
            result.return_data=Bytes::new();
            assert!(word(&result).is_none());
        }
    }
}
