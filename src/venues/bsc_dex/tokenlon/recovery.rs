//! Relayer может оплатить то же подписанное ERC20-поручение из другой внешней транзакции.
//! Отсутствие выплаты доказывается на финализированном блоке после срока, по всем четырём темам PMM.
use super::{settlement::Saved,wire};
use crate::{
    core::types::{ChainTx,Fee,Side,TradeRequest,TradeResult,TradeStatus,TxOutcome},
    gateway::{errors::{ErrorCode,GatewayResult},pending::PendingRef,settlement_intent},
    venues::bsc_dex::{ctx::Ctx,exec::swap_ctx::SwapCtx,gas_ticker,node::rpc,num,parts::Parts,resolve,send},
};
use alloy_primitives::{B256,U64,U256};
use alloy::rpc::types::Log;
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Header{number:U64,timestamp:U64,hash:B256}
enum Payment{Open,None,Found(B256)}
async fn lookup(c:&Ctx,o:&Saved)->GatewayResult<Payment>{
    let bad=||c.err(ErrorCode::OutcomeUnknown,"Tokenlon: история исполнения личного поручения не подтверждена");
    let end:Header=rpc::call(c,"eth_getBlockByNumber",json!(["finalized",false])).await?;
    let (mut lo,mut hi)=(o.from_block,end.number.to::<u64>());
    if hi<lo{return Ok(Payment::Open);}
    let expired=end.timestamp.to::<u64>()>=o.order.expiry;
    // Последовательность Ethereum timestamps строго возрастает. При позднем восстановлении
    // верхняя граница — первый блок после срока, а не все блоки вплоть до сегодняшнего дня.
    if expired {
        while lo<hi {
            let mid=lo+(hi-lo)/2;
            let h:Header=rpc::call(c,"eth_getBlockByNumber",json!([U64::from(mid),false])).await?;
            if h.number.to::<u64>()!=mid{return Err(bad());}
            if h.timestamp.to::<u64>()>=o.order.expiry {hi=mid;} else {lo=mid+1;}
        }
    }
    let mut cursor=o.from_block;
    let size=c.int("dex.logs_block_batch")?;
    if size==0{return Err(bad());}
    let mut found=None;
    while cursor<=hi {
        let last=cursor.saturating_add(size-1).min(hi);
        let rows:Vec<Log>=rpc::call(c,"eth_getLogs",json!([{"fromBlock":U64::from(cursor),"toBlock":U64::from(last),
            "address":wire::PMM,"topics":o.order.fill_topics()}])).await?;
        for row in rows {
            if row.removed || row.address()!=wire::PMM || row.topics()!=o.order.fill_topics() ||
                row.block_number.is_none_or(|n|n<cursor || n>last) || found.is_some(){return Err(bad());}
            found=Some(row.transaction_hash.ok_or_else(bad)?);
        }
        if last==hi{break;}cursor=last+1;
    }
    let still:Header=rpc::call(c,"eth_getBlockByNumber",json!([end.number,false])).await?;
    if still.hash!=end.hash{return Err(bad());}
    Ok(match found{Some(h)=>Payment::Found(h),None if expired=>Payment::None,None=>Payment::Open})
}
pub(in crate::venues::bsc_dex) async fn expired_unspent(c:&Ctx,s:&Saved)->bool{
    // При msg.value деньги с владельца может снять только его собственный внешний вызов.
    s.order.terms.input==wire::WETH || matches!(lookup(c,s).await,Ok(Payment::None))
}

pub(in crate::venues::bsc_dex) async fn actual(c:&Ctx,s:&Saved,tx:&ChainTx,reference:&str)->GatewayResult<(U256,U256)>{
    let bad=||c.err(ErrorCode::OutcomeUnknown,"Tokenlon: отсутствует квитанция личного поручения или собственных затрат");
    let id:B256=reference.parse().map_err(|_|bad())?;
    let receipt=send::receipt::read(c,id).await?.ok_or_else(bad)?;
    let mut paid=s.order.clone();
    // Минимум сохранён в исходном поручении. Для учёта измеряется и действительный недобор.
    paid.terms.minimum=U256::from(1);
    if id==tx.hash {return paid.included(c,id,tx.nonce,s.calldata,&receipt).await;}
    if s.order.terms.input==wire::WETH{return Err(bad());}
    let (ours,cancel)=match tx.outcome{
        TxOutcome::Reverted=>(tx.hash,false),
        TxOutcome::NotIncluded=>(tx.cancel_hash.ok_or_else(bad)?,true),
        _=>return Err(bad()),
    };
    let expense=send::receipt::read(c,ours).await?.ok_or_else(bad)?;
    if cancel{
        s.signed().cancelled(c,tx.hash,ours,tx.nonce,&expense).await?;
        if expense.gas_bnb()!=tx.cancel_gas_bnb{return Err(bad());}
    }else{
        if expense.status!=U64::ZERO || expense.gas_bnb()!=tx.gas_bnb{return Err(bad());}
        paid.transaction_matches(c,ours,tx.nonce,s.calldata,&expense).await?;
    }
    paid.relayed(c,id,s.calldata,&receipt).await
}

pub(in crate::venues::bsc_dex) async fn cash_for_record(p:&Parts,t:&TradeResult,pend:&PendingRef)->GatewayResult<rust_decimal::Decimal>{
    let bad=||p.ctx.err(ErrorCode::OutcomeUnknown,"Не подтверждён фактический оборот Tokenlon").pending(pend.clone());
    let c=SwapCtx::of(pend).ok_or_else(bad)?;
    let s=c.tokenlon.as_deref().ok_or_else(bad)?;
    let tx=t.chain.as_ref().ok_or_else(bad)?;
    let (d,r)=actual(&p.ctx,s,tx,&t.venue_ref).await?;
    let (_,dec)=c.decs.ok_or_else(bad)?;
    num::to_decimal(if t.request.side==Side::Buy{d}else{r},dec).ok_or_else(bad)
}

/// Some(итог) — личная позиция действительно закрыта внешним исполнением. None — обычная собственная сверка.
pub(in crate::venues::bsc_dex) async fn recover(p:&Parts,req:&TradeRequest,pend:&PendingRef)->GatewayResult<Option<TradeResult>>{
    let Some(parse)=SwapCtx::of(pend) else{return Ok(None)};
    let Some(s)=parse.tokenlon.as_deref() else{return Ok(None)};
    let c=&p.ctx;
    let unknown=||c.err(ErrorCode::OutcomeUnknown,"Tokenlon: собственный nonce и выплата личной заявки ещё сверяются").pending(pend.clone());
    let mut own=parse.chain.clone().ok_or_else(unknown)?;
    if !s.wallet(c) || pend.nonce!=Some(own.nonce) || pend.venue_ref.as_ref()!=Some(&own.hash.to_string())
        || req.route.as_ref().is_none_or(|q|q.mode!=crate::core::types::RouteMode::Rfq(s.order.tx_hash)){return Err(unknown());}
    if s.order.terms.input==wire::WETH{return Ok(None);}
    if let Some(r)=send::receipt::read(c,own.hash).await? {
        s.order.transaction_matches(c,own.hash,own.nonce,s.calldata,&r).await?;
        if r.status!=U64::ZERO{return Ok(None);}
        own.outcome=TxOutcome::Reverted;
        own.block=Some(r.block_number.to());
        own.gas_used=Some(r.gas_used.to());
        own.gas_bnb=Some(r.gas_bnb().ok_or_else(unknown)?);
    } else {
        let mut verified=false;
        for hash in own.cancel_hash.into_iter().chain(parse.cancels.into_iter().flatten()) {
            if let Some(r)=send::receipt::read(c,hash).await?{
                s.signed().cancelled(c,own.hash,hash,own.nonce,&r).await?;
                own.outcome=TxOutcome::NotIncluded;own.cancel_hash=Some(hash);
                own.cancel_gas_bnb=Some(r.gas_bnb().ok_or_else(unknown)?);
                verified=true;break;
            }
        }
        if !verified{return Ok(None);}
    }
    let external=match lookup(c,s).await?{
        Payment::Found(h) if h!=own.hash=>h,
        Payment::None=>return Ok(None),
        _=>return Err(unknown()),
    };
    let (spent,received)=actual(c,s,&own,&external.to_string()).await?;
    let (co,ca)=if req.side==Side::Buy{(received,spent)}else{(spent,received)};
    let (db,dc)=parse.decs.ok_or_else(unknown)?;
    let qty=num::to_decimal(co,db).ok_or_else(unknown)?;
    let cash=num::to_decimal(ca,dc).ok_or_else(unknown)?;
    let gas=own.gas_bnb.unwrap_or_default()+own.cancel_gas_bnb.unwrap_or_default();
    let fees=Fee::single(gas,&gas_ticker::sent(c)?);
    let status=if qty<req.qty {TradeStatus::PartiallyFilled}else{TradeStatus::Filled};
    let note=if received<s.order.terms.minimum {"; фактическая выплата ниже согласованной границы"}else{""};
    let _=resolve::release_if_ours(p,own.hash,Some(own.nonce),false).await;
    settlement_intent::note_gross(Some(cash));
    Ok(Some(TradeResult{request:req.clone(),status,filled_qty:qty,avg_price:cash.checked_div(qty).ok_or_else(unknown)?,
        fees,venue_ref:external.to_string(),chain:Some(own),finished_at:Utc::now(),
        executor:Some(format!("{}; личная заявка оплачена ретранслятором{note}",super::LABEL))}))
}
