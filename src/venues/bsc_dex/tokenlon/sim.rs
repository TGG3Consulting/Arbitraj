//! Одна полная выплата без подмены балансов. ETH у PMMV5 передаётся через msg.value.
use super::{receipt::Proof, wire};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{check::simulate::CallResult, ctx::Ctx, exec::signed_call, node::rpc, pools::solidly_classic_factory as binding},
};
use alloy_primitives::{Address, B256, Bytes, U64, U256, address, b256, keccak256};
use serde::Deserialize;
use serde_json::{Value, json};

const PROXY_IMPL:Address=address!("0b9f13ffab8448089f50073cf24bbe5c7bd8675a");
const PROXY_RUNTIME:B256=b256!("abf73cab47631c9363fa11b421585b9edc25458176a4e00292846ed8a45b3c1a");
const IMPL_RUNTIME:B256=b256!("db3bf7e4d3bea9449bdaaf049140386c9c0d57c1d9fcb962a88310b002fc70a5");
const PMM_RUNTIME:B256=b256!("d93c60d37531f7091af0f56e1ead05e61b26fffe245618ea084d807b00a5ab90");
const ZERO_RUNTIME:B256=b256!("1eb08f4905a226eb84c664ee73b6d81cd1a40720e6b3dd946591500a959aeefa");
#[derive(Deserialize)]
struct Header {hash:B256, number:U64}
#[derive(Deserialize)]
#[serde(rename_all="camelCase")]
struct Block {parent_hash:B256, number:U64, timestamp:U64, calls:Vec<CallResult>}

fn balance(who:Address, asset:Address, multicall:Address)->Value {
    let (to,name)=if asset==wire::WETH {(multicall,"getEthBalance(address)")}else{(asset,"balanceOf(address)")};
    let mut data=keccak256(name.as_bytes())[..4].to_vec();
    data.extend_from_slice(&[0;12]);
    data.extend_from_slice(who.as_slice());
    json!({"from":who,"to":to,"data":Bytes::from(data)})
}

pub(super) fn value(p:&Proof)->U256 {
    if p.terms.input==wire::WETH {p.terms.spent} else {U256::ZERO}
}

pub(super) async fn run(ctx:&Ctx,proof:&Proof,data:&Bytes)->GatewayResult<(u64,u64)> {
    let reject=||ctx.err(ErrorCode::QuoteMismatch,"Tokenlon: симуляция не подтвердила точные расходы, зачисление и канонический исходный блок");
    let (head,chain):(Header,U64)=tokio::try_join!(
        rpc::call(ctx,"eth_getBlockByNumber",json!(["latest",false])),
        rpc::call(ctx,"eth_chainId",json!([])))?;
    if chain.to::<u64>()!=1 || ctx.chain_id()!=Some(1) {return Err(reject());}
    let tag=json!({"blockHash":head.hash,"requireCanonical":true});
    let t=&proof.terms;
    let actor:Bytes=rpc::call(ctx,"eth_getCode",json!([t.payer,&tag])).await?;
    if !actor.is_empty() {return Err(reject());}
    tokio::try_join!(
        binding::bind(ctx,wire::USER_PROXY,PROXY_RUNTIME,Some((PROXY_IMPL,IMPL_RUNTIME)),&tag),
        binding::plain(ctx,wire::PMM,PMM_RUNTIME,&tag),
        binding::plain(ctx,wire::ZERO_EXCHANGE,ZERO_RUNTIME,&tag),
        super::assets::exact_transfers(ctx,t.input,t.output,&tag))?;
    let mc=ctx.text("dex.multicall3")?.parse::<Address>().map_err(|_|reject())?;
    let input=balance(t.payer,t.input,mc);
    let output=balance(t.recipient,t.output,mc);
    let action=json!({"from":t.payer,"to":wire::USER_PROXY,"data":data,"value":value(proof)});
    // В запросе находится личная подпись fill. Ошибка транспорта не должна содержать исходные params.
    let blocks:Vec<Block>=rpc::call(ctx,"eth_simulateV1",json!([{"blockStateCalls":[{
        "calls":[&input,&output,action,&input,&output]}]},&tag])).await.map_err(|_|reject())?;
    let [b]=blocks.as_slice() else{return Err(reject())};
    let [i0,o0,swap,i1,o1]=b.calls.as_slice() else{return Err(reject())};
    let word=|c:&CallResult| (c.ok() && c.return_data.len()==32).then(||U256::from_be_slice(&c.return_data));
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
