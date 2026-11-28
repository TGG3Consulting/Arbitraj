//! Исполнение RFQT через существующую очередь nonce и разбор квитанций DEX.
#[path="proof_chain.rs"]
mod proof_chain;
use super::super::{exec, num, parts::Parts, send, wallet};
use super::{routing, sim, wire,dispatch::{Dispatch,Gate}};
use crate::core::types::{RouteMode, Side, TradeRequest, TradeResult, TxOutcome};
use super::model::Request;
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    events::MarketEvent,
    settlement_intent as intent,
};
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, U256};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Публичные поля, достаточные для подтверждения сделки по квитанции после перезапуска.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptProof {
    #[serde(default,skip_serializing_if="Option::is_none")]
    original:Option<proof_chain::Original>,
    pool: Address,
    trader: Address,
    input: Address,
    output: Address,
    txid: B256,
    amount: U256,
    proceeds: U256,
}
impl ReceiptProof {
    pub(in crate::venues::bsc_dex) fn matches(&self, receipt: &send::receipt::Receipt) -> bool {
        let req = Request {
            trader: self.trader,
            input: self.input,
            output: self.output,
            amount: None,
            amount_tokens: None,
        };
        let mut logs = receipt
            .logs
            .iter()
            .filter(|l| l.address() == self.pool && l.topics() == [wire::Trade::SIGNATURE_HASH]);
        let event = logs.next().is_some_and(|l| {
            sim::event_matches(
                &l.inner.data.data,
                &req,
                self.txid,
                self.amount,
                self.proceeds,
            )
        }) && logs.next().is_none();
        // Штатный result использует именно эти два направления Transfer. Лишние движения кошелька расходятся с подписанной ценой.
        event
            && exec::fill::moved(receipt, self.input, Some(self.trader), None) == self.amount
            && exec::fill::moved(receipt, self.output, None, Some(self.trader)) == self.proceeds
            && exec::fill::moved(receipt, self.input, None, Some(self.trader)).is_zero()
            && exec::fill::moved(receipt, self.output, Some(self.trader), None).is_zero()
    }
}

/// При подтверждённом откате или принятой отмене освобождает логический этап для законного повтора. Утерянный ответ и неясный nonce сохраняют исходный маркер.
pub(in crate::venues::bsc_dex) async fn failed(
    p: &Parts,
    r: &TradeRequest,
    e: crate::gateway::errors::GatewayError,
) -> crate::gateway::errors::GatewayError {
    if !matches!(r.route.as_ref().map(|v| v.mode), Some(RouteMode::Rfq(_) | RouteMode::ZeroEx(_) | RouteMode::Raindex(Some(_)))) {
        return e;
    }
    let Some(tx) = e.tx.as_ref().filter(|t| {
        (t.outcome == TxOutcome::Reverted && t.block.is_some())
            || (t.outcome == TxOutcome::NotIncluded && t.cancel_hash.is_some())
    }) else {
        return e;
    };
    let Ok(k) = key(p, r) else {
        return e;
    };
    let db = p.ctx.journal.db();
    let Ok(Some(entry)) = intent::load(db, &k).await else {
        return e;
    };
    let Some(c)=exec::swap_ctx::SwapCtx::of(&entry.lookup) else{return e;};
    if [c.rfq.is_some(),c.dexalot.is_some(),c.tokenlon.is_some(),c.airswap.is_some(),c.zeroex.is_some(),c.textile.is_some(),c.gladius.is_some(),c.raindex.is_some(),c.epsilon.is_some(),c.native_rfq.is_some()].into_iter().filter(|v|*v).count()!=1{return e;}
    if let Some(proof)=c.rfq{
        let Some(original)=c.chain.as_ref() else{return e;};
        if original.hash!=tx.hash || original.nonce!=tx.nonce || entry.lookup.nonce!=Some(tx.nonce) {return e;}
        let verified=if tx.outcome==TxOutcome::Reverted{
            let Ok(Some(r))=send::receipt::read(&p.ctx,tx.hash).await else{return e;};
            r.status.to::<u64>()==0 && proof.included(&p.ctx,tx.hash,tx.nonce,&r).await.is_ok()
        }else if let Some(cancel)=tx.cancel_hash{
            proof.cancellation(&p.ctx,tx.hash,cancel,tx.nonce).await.is_ok()
        }else{false};
        if !verified{return e;}
    }
    if let Some(g)=c.gladius{
        if tx.outcome!=TxOutcome::Reverted{return e;}
        let Ok(Some(receipt))=send::receipt::read(&p.ctx,tx.hash).await else{return e;};
        if receipt.status.to::<u64>()!=0 || g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.is_err(){return e;}
    }
    if let Some(g)=c.dexalot{
        if tx.outcome!=TxOutcome::Reverted{return e;}
        let Ok(Some(receipt))=send::receipt::read(&p.ctx,tx.hash).await else{return e;};
        if receipt.status.to::<u64>()!=0 || g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.is_err(){return e;}
    }
    if let Some(g)=c.tokenlon{
        let Some(original)=c.chain.as_ref() else {return e;};
        if original.hash!=tx.hash || original.nonce!=tx.nonce || entry.lookup.nonce!=Some(tx.nonce){return e;}
        if tx.outcome==TxOutcome::Reverted{
            let Ok(Some(receipt))=send::receipt::read(&p.ctx,tx.hash).await else{return e;};
            if receipt.status.to::<u64>()!=0 || g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.is_err(){return e;}
        }else if let Some(cancel)=tx.cancel_hash{
            let Ok(Some(r))=send::receipt::read(&p.ctx,cancel).await else{return e;};
            if g.signed().cancelled(&p.ctx,tx.hash,cancel,tx.nonce,&r).await.is_err(){return e;}
        }else{return e;}
    }
    if let Some(g)=c.airswap{
        if tx.outcome!=TxOutcome::Reverted{return e;}
        let Ok(Some(receipt))=send::receipt::read(&p.ctx,tx.hash).await else{return e;};
        if receipt.status.to::<u64>()!=0 || g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.is_err(){return e;}
    }
    if let Some(proof)=c.zeroex.as_ref(){
        if c.chain.as_ref().is_none_or(|v|v.hash!=tx.hash || v.nonce!=tx.nonce) || entry.lookup.nonce!=Some(tx.nonce){return e;}
        let hash=tx.cancel_hash.filter(|_|tx.outcome==TxOutcome::NotIncluded).unwrap_or(tx.hash);
        let Ok(Some(receipt))=send::receipt::read(&p.ctx,hash).await else{return e;};
        let verified=if tx.outcome==TxOutcome::Reverted{
            receipt.status.to::<u64>()==0 && proof.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.is_ok()
        }else{proof.call.cancelled(&p.ctx,tx.hash,hash,tx.nonce,&receipt).await.is_ok()};
        if !verified{return e;}
    }
    if let Some(g)=c.textile{
        if tx.outcome!=TxOutcome::Reverted{return e;}
        let Ok(Some(receipt))=send::receipt::read(&p.ctx,tx.hash).await else{return e;};
        if receipt.status.to::<u64>()!=0 || g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.is_err(){return e;}
    }
    if let Some(g)=c.epsilon{
        if tx.outcome!=TxOutcome::Reverted{return e;}
        let Ok(Some(receipt))=send::receipt::read(&p.ctx,tx.hash).await else{return e;};
        if receipt.status.to::<u64>()!=0 || g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.is_err(){return e;}
    }
    if let Some(g)=c.raindex{
        if tx.outcome!=TxOutcome::Reverted{return e;}
        let Ok(Some(receipt))=send::receipt::read(&p.ctx,tx.hash).await else{return e;};
        if receipt.status.to::<u64>()!=0 || g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.is_err(){return e;}
    }
    if let Some(n)=c.native_rfq{
        if !c.chain.is_some_and(|v|v.hash==tx.hash && v.nonce==tx.nonce) || entry.lookup.nonce!=Some(tx.nonce){return e;}
        let hash=tx.cancel_hash.filter(|_|tx.outcome==TxOutcome::NotIncluded).unwrap_or(tx.hash);
        let Ok(Some(receipt))=send::receipt::read(&p.ctx,hash).await else{return e};
        let verified=if tx.outcome==TxOutcome::Reverted {
            receipt.status.to::<u64>()==0 && n.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.is_ok()
        } else {
            n.signed().cancelled(&p.ctx,tx.hash,hash,tx.nonce,&receipt).await.is_ok()
        };
        if !verified{return e}
    }
    if entry.result.is_none() && entry.lookup.venue_ref.as_ref() == Some(&tx.hash.to_string()) {
        let _ = intent::archive_attempt(db, &k, &entry, "Квитанция подтверждает отсутствие обмена")
            .await;
    }
    e
}

/// Метка охватывает смену RFQ в одном денежном шаге. Повтор с другим размером после неопределённой отправки тоже требует сверки.
pub(super) fn key(p: &Parts, r: &TradeRequest) -> GatewayResult<String> {
    let account = p
        .ctx
        .account
        .as_deref()
        .ok_or_else(|| routing::no(p, "Для Hashflow требуется счёт сделки"))?;
    if r.deal_id.is_empty() {
        return Err(routing::no(
            p,
            "Для исполнения Hashflow нужен идентификатор сделки",
        ));
    }
    let coin = r
        .asset
        .contract
        .ok_or_else(|| routing::no(p, "У монеты отсутствует контракт"))?;
    let venue = p.ctx.place.id;
    Ok(format!(
        "hashflow/{venue}/{account}/{coin}/{:?}/{}",
        r.side, r.deal_id
    ))
}

pub(super) async fn prior(p: &Parts, r: &TradeRequest) -> GatewayResult<Option<TradeResult>> {
    let db = p.ctx.journal.db();
    let Some(entry) = intent::load(db, &key(p, r)?)
        .await
        .map_err(|e| p.ctx.err(ErrorCode::Internal, e))?
    else {
        return Ok(None);
    };
    let saved = exec::swap_ctx::SwapCtx::of(&entry.lookup);
    if saved.as_ref().is_some_and(|c| c.dexalot.as_ref().is_some_and(|n|!n.wallet(&p.ctx)) || c.tokenlon.as_ref().is_some_and(|n|!n.wallet(&p.ctx)) || c.airswap.as_ref().is_some_and(|n|!n.wallet(&p.ctx)) || c.zeroex.as_ref().is_some_and(|n|!n.wallet(&p.ctx)) || c.textile.as_ref().is_some_and(|n|!n.wallet(&p.ctx)) || c.native_rfq.as_ref().is_some_and(|n|!n.wallet(&p.ctx)) || c.epsilon.as_ref().is_some_and(|g|!g.wallet(&p.ctx)) || c.raindex.as_ref().is_some_and(|g|!g.wallet(&p.ctx)) || c.gladius.as_ref().is_some_and(|g|!g.wallet(&p.ctx))){
        return Err(p.ctx.err(ErrorCode::OutcomeUnknown,"Сначала требуется сверить первоначальный счёт и сеть Native RFQ").pending(entry.lookup));
    }
    if let Some(intent::Outcome::Order(x)) = entry.result {
        if let Some(proof)=saved.as_ref().and_then(|c|c.rfq.as_ref()){
            let unknown=||p.ctx.err(ErrorCode::OutcomeUnknown,"Hashflow: требуется исходная сеть и первоначальный кошелёк завершённого поручения").pending(entry.lookup.clone());
            if wallet::signer::address(&p.ctx).ok()!=Some(proof.trader){return Err(unknown());}
            let tx=x.trade.chain.as_ref().ok_or_else(unknown)?;
            let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|unknown())?.ok_or_else(unknown)?;
            if !proof_chain::same_fill(tx,&receipt){return Err(unknown());}
            proof.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|unknown())?;
        }
        if let Some(proof)=saved.as_ref().and_then(|c|c.dexalot.as_ref()){
            let unknown=||p.ctx.err(ErrorCode::OutcomeUnknown,"Dexalot: требуется каноническая квитанция исходного платежа").pending(entry.lookup.clone());
            let tx=x.trade.chain.as_ref().ok_or_else(unknown)?;
            let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|unknown())?.ok_or_else(unknown)?;
            if !proof_chain::same_fill(tx,&receipt){return Err(unknown());}
            proof.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|unknown())?;
            let (d,c)=proof.amounts(&receipt.logs).ok_or_else(unknown)?;
            let (asset_dec,cash_dec)=saved.as_ref().and_then(|s|s.decs).ok_or_else(unknown)?;
            let (asset,cash)=if r.side==Side::Buy{(c,d)}else{(d,c)};
            if num::to_decimal(asset,asset_dec)!=Some(x.trade.filled_qty)||num::to_decimal(cash,cash_dec)!=Some(x.gross_quote){return Err(unknown());}
        }
        if let Some(proof)=saved.as_ref().and_then(|c|c.zeroex.as_ref()){
            let unknown=||p.ctx.err(ErrorCode::OutcomeUnknown,"0x: требуется каноническая квитанция исходного платежа").pending(entry.lookup.clone());
            let tx=x.trade.chain.as_ref().ok_or_else(unknown)?;
            let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|unknown())?.ok_or_else(unknown)?;
            if !proof_chain::same_fill(tx,&receipt){return Err(unknown());}
            proof.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|unknown())?;
            let (d,c)=proof.amounts(&receipt.logs).ok_or_else(unknown)?;
            let (asset_dec,cash_dec)=saved.as_ref().and_then(|s|s.decs).ok_or_else(unknown)?;
            let (asset,cash)=if r.side==Side::Buy{(c,d)}else{(d,c)};
            if num::to_decimal(asset,asset_dec)!=Some(x.trade.filled_qty)||num::to_decimal(cash,cash_dec)!=Some(x.gross_quote){return Err(unknown());}
        }
        if let Some(proof)=saved.as_ref().and_then(|c|c.tokenlon.as_ref()){
            let unknown=||p.ctx.err(ErrorCode::OutcomeUnknown,"Tokenlon: требуется каноническая квитанция исходного платежа").pending(entry.lookup.clone());
            let tx=x.trade.chain.as_ref().ok_or_else(unknown)?;
            let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|unknown())?.ok_or_else(unknown)?;
            if !proof_chain::same_fill(tx,&receipt){return Err(unknown());}
            proof.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|unknown())?;
            let (d,c)=proof.amounts(&receipt.logs).ok_or_else(unknown)?;
            let (asset_dec,cash_dec)=saved.as_ref().and_then(|s|s.decs).ok_or_else(unknown)?;
            let (asset,cash)=if r.side==Side::Buy{(c,d)}else{(d,c)};
            if num::to_decimal(asset,asset_dec)!=Some(x.trade.filled_qty)||num::to_decimal(cash,cash_dec)!=Some(x.gross_quote){return Err(unknown());}
        }
        if let Some(proof)=saved.as_ref().and_then(|c|c.airswap.as_ref()){
            let unknown=||p.ctx.err(ErrorCode::OutcomeUnknown,"AirSwap: требуется каноническая квитанция исходного платежа").pending(entry.lookup.clone());
            let tx=x.trade.chain.as_ref().ok_or_else(unknown)?;
            let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|unknown())?.ok_or_else(unknown)?;
            if !proof_chain::same_fill(tx,&receipt){return Err(unknown());}
            proof.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|unknown())?;
            let (d,c)=proof.amounts(&receipt.logs).ok_or_else(unknown)?;
            let (asset_dec,cash_dec)=saved.as_ref().and_then(|s|s.decs).ok_or_else(unknown)?;
            let (asset,cash)=if r.side==Side::Buy{(c,d)}else{(d,c)};
            if num::to_decimal(asset,asset_dec)!=Some(x.trade.filled_qty)||num::to_decimal(cash,cash_dec)!=Some(x.gross_quote){return Err(unknown());}
        }
        if let Some(proof)=saved.as_ref().and_then(|c|c.textile.as_ref()){
            let unknown=||p.ctx.err(ErrorCode::OutcomeUnknown,"Textile: требуется каноническая квитанция исходного платежа").pending(entry.lookup.clone());
            let tx=x.trade.chain.as_ref().ok_or_else(unknown)?;
            let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|unknown())?.ok_or_else(unknown)?;
            if !proof_chain::same_fill(tx,&receipt){return Err(unknown());}
            proof.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|unknown())?;
            let (d,c)=proof.amounts(&receipt.logs).ok_or_else(unknown)?;
            let (_,dec)=saved.as_ref().and_then(|s|s.decs).ok_or_else(unknown)?;
            if crate::venues::bsc_dex::num::to_decimal(if r.side==Side::Buy{d}else{c},dec)!=Some(x.gross_quote){return Err(unknown());}
        }
        if let Some(proof)=saved.as_ref().and_then(|c|c.epsilon.as_ref()){
            let unknown=||p.ctx.err(ErrorCode::OutcomeUnknown,"Epsilon: требуется каноническая квитанция исходного платежа").pending(entry.lookup.clone());
            let tx=x.trade.chain.as_ref().ok_or_else(unknown)?;
            let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|unknown())?.ok_or_else(unknown)?;
            if !proof_chain::same_fill(tx,&receipt){return Err(unknown());}
            proof.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|unknown())?;
            let (d,c)=proof.amounts(&receipt.logs).ok_or_else(unknown)?;
            let (_,dec)=saved.as_ref().and_then(|s|s.decs).ok_or_else(unknown)?;
            if crate::venues::bsc_dex::num::to_decimal(if r.side==Side::Buy{d}else{c},dec)!=Some(x.gross_quote){return Err(unknown());}
        }
        if let Some(proof)=saved.as_ref().and_then(|c|c.raindex.as_ref()){
            let unknown=||p.ctx.err(ErrorCode::OutcomeUnknown,"Raindex: требуется каноническая квитанция исходного платежа").pending(entry.lookup.clone());
            let tx=x.trade.chain.as_ref().ok_or_else(unknown)?;
            let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|unknown())?.ok_or_else(unknown)?;
            if !proof_chain::same_fill(tx,&receipt){return Err(unknown());}
            proof.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|unknown())?;
            let (d,c)=proof.amounts(&receipt.logs).ok_or_else(unknown)?;
            let (_,dec)=saved.as_ref().and_then(|s|s.decs).ok_or_else(unknown)?;
            if crate::venues::bsc_dex::num::to_decimal(if r.side==Side::Buy{d}else{c},dec)!=Some(x.gross_quote){return Err(unknown());}
        }
        if x.trade.request.qty == r.qty {
            return Ok(Some(x.trade));
        }
        return Err(routing::no(
            p,
            "Этот денежный шаг Hashflow уже завершён с другим объёмом",
        ));
    }
    Err(p
        .ctx
        .err(
            ErrorCode::OutcomeUnknown,
            "Сначала требуется сверить ранее подготовленную транзакцию Hashflow",
        )
        .pending(entry.lookup))
}

/// Фиксируется только подтверждённый по квитанции итог, после штатной проверки баланса ядром.
pub(in crate::venues::bsc_dex) async fn record(
    p: &Parts,
    t: TradeResult,
) -> GatewayResult<TradeResult> {
    if !matches!(
        t.request.route.as_ref().map(|r| r.mode),
        Some(RouteMode::Rfq(_) | RouteMode::ZeroEx(_) | RouteMode::Raindex(Some(_)))
    ) {
        return Ok(t);
    }
    let db = p.ctx.journal.db();
    let k = key(p, &t.request)?;
    let err = |_| {
        p.ctx
            .err(
                ErrorCode::OutcomeUnknown,
                "Не удалось подтвердить запись об уже отправленном Hashflow",
            )
            .pending(
                t.chain
                    .as_ref()
                    .map(|x| exec::swap_ctx::seed(x, None))
                    .unwrap_or_default(),
            )
    };
    let entry = intent::load(db, &k)
        .await
        .map_err(err)?
        .ok_or_else(|| err(String::new()))?;
    let err = |_:String| p.ctx.err(ErrorCode::OutcomeUnknown,"Не удалось сверить первоначальную отправку RFQ").pending(entry.lookup.clone());
    if t.chain
        .as_ref()
        .is_none_or(|tx| entry.lookup.venue_ref.as_deref() != Some(tx.hash.to_string().as_str()))
    {
        return Err(p
            .ctx
            .err(
                ErrorCode::OutcomeUnknown,
                "Хеш результата расходится с сохранённой отправкой",
            )
            .pending(entry.lookup));
    }
    if matches!(entry.result,Some(intent::Outcome::Order(_))) {
        return prior(p,&t.request).await?.ok_or_else(||err(String::new()));
    }
    let parse = exec::swap_ctx::SwapCtx::of(&entry.lookup).ok_or_else(|| err(String::new()))?;
    let (_, qd)=parse.decs.ok_or_else(||err(String::new()))?;
    let gross=if let Some(g)=parse.gladius{
        let tx=t.chain.as_ref().ok_or_else(||err(String::new()))?;
        let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|err(String::new()))?.ok_or_else(||err(String::new()))?;
        g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|err(String::new()))?;
        let (debit,credit)=g.amounts(&receipt.logs).ok_or_else(||err(String::new()))?;
        if t.request.side==Side::Buy{debit}else{credit}
    }else if let Some(g)=parse.dexalot{
        let tx=t.chain.as_ref().ok_or_else(||err(String::new()))?;
        let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|err(String::new()))?.ok_or_else(||err(String::new()))?;
        g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|err(String::new()))?;
        let (debit,credit)=g.amounts(&receipt.logs).ok_or_else(||err(String::new()))?;
        if t.request.side==Side::Buy{debit}else{credit}
    }else if let Some(g)=parse.tokenlon{
        let tx=t.chain.as_ref().ok_or_else(||err(String::new()))?;
        let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|err(String::new()))?.ok_or_else(||err(String::new()))?;
        g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|err(String::new()))?;
        let (debit,credit)=g.amounts(&receipt.logs).ok_or_else(||err(String::new()))?;
        if t.request.side==Side::Buy{debit}else{credit}
    }else if let Some(g)=parse.airswap{
        let tx=t.chain.as_ref().ok_or_else(||err(String::new()))?;
        let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|err(String::new()))?.ok_or_else(||err(String::new()))?;
        g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|err(String::new()))?;
        let (debit,credit)=g.amounts(&receipt.logs).ok_or_else(||err(String::new()))?;
        if t.request.side==Side::Buy{debit}else{credit}
    }else if let Some(g)=parse.zeroex{
        let tx=t.chain.as_ref().ok_or_else(||err(String::new()))?;
        let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|err(String::new()))?.ok_or_else(||err(String::new()))?;
        g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|err(String::new()))?;
        let (debit,credit)=g.amounts(&receipt.logs).ok_or_else(||err(String::new()))?;
        if t.request.side==Side::Buy{debit}else{credit}
    }else if let Some(g)=parse.textile{
        let tx=t.chain.as_ref().ok_or_else(||err(String::new()))?;
        let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|err(String::new()))?.ok_or_else(||err(String::new()))?;
        g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|err(String::new()))?;
        let (debit,credit)=g.amounts(&receipt.logs).ok_or_else(||err(String::new()))?;
        if t.request.side==Side::Buy{debit}else{credit}
    }else if let Some(g)=parse.epsilon{
        let tx=t.chain.as_ref().ok_or_else(||err(String::new()))?;
        let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|err(String::new()))?.ok_or_else(||err(String::new()))?;
        g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|err(String::new()))?;
        let (debit,credit)=g.amounts(&receipt.logs).ok_or_else(||err(String::new()))?;
        if t.request.side==Side::Buy{debit}else{credit}
    }else if let Some(g)=parse.raindex{
        let tx=t.chain.as_ref().ok_or_else(||err(String::new()))?;
        let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|err(String::new()))?.ok_or_else(||err(String::new()))?;
        g.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|err(String::new()))?;
        let (debit,credit)=g.amounts(&receipt.logs).ok_or_else(||err(String::new()))?;
        if t.request.side==Side::Buy{debit}else{credit}
    }else if let Some(native)=parse.native_rfq{
        let tx=t.chain.as_ref().ok_or_else(||err(String::new()))?;
        let receipt=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|err(String::new()))?.ok_or_else(||err(String::new()))?;
        native.included(&p.ctx,tx.hash,tx.nonce,&receipt).await.map_err(|_|err(String::new()))?;
        if t.request.side==Side::Buy{native.sold()}else{native.matches(&receipt).ok_or_else(||err(String::new()))?}
    }else{
        let proof=parse.rfq.ok_or_else(||err(String::new()))?;
        let tx=t.chain.as_ref().ok_or_else(||err(String::new()))?;
        let r=send::receipt::read(&p.ctx,tx.hash).await.map_err(|_|err(String::new()))?.ok_or_else(||err(String::new()))?;
        if !proof_chain::same_fill(tx,&r){return Err(err(String::new()));}
        proof.included(&p.ctx,tx.hash,tx.nonce,&r).await.map_err(|_|err(String::new()))?;
        if t.request.side==Side::Buy{proof.amount}else{proof.proceeds}
    };
    let gross=super::super::num::to_decimal(gross,qd).ok_or_else(||err(String::new()))?;
    let s = intent::SpotFill {
        trade: t.clone(),
        gross_quote: gross,
    };
    intent::confirm(db, &k, &entry, intent::Outcome::Order(Box::new(s)))
        .await
        .map_err(err)?;
    Ok(t)
}

pub(super) async fn trade(p: &Parts, req: TradeRequest, id: B256) -> GatewayResult<TradeResult> {
    if req.venue != p.ctx.venue() || req.qty <= Decimal::ZERO || req.worst_price <= Decimal::ZERO {
        return Err(routing::no(
            p,
            "Запрос не соответствует площадке или денежному пределу",
        ));
    }
    if let Some(trade) = prior(p, &req).await? {
        return Ok(trade);
    }
    // A recovery lookup ends here without constructing the futures for signing or exchange.
    Box::pin(send_selected(p, req, id)).await
}

async fn send_selected(p: &Parts, req: TradeRequest, id: B256) -> GatewayResult<TradeResult> {
    let ctx = &p.ctx;
    ctx.bound()?;
    let who = wallet::signer::address(ctx)?;
    let mode = req.route.as_ref().map(|r| r.mode);
    let (result, fallback_fee) = if mode==Some(RouteMode::ZeroEx(id)) {
        let ticket=p.zeroex.take(who,id).ok_or_else(||routing::no(p,"Нужна новая подтверждённая заявка 0x"))?;
        return Box::pin(super::super::zeroex::trade(p,req,&ticket)).await;
    }else if mode == Some(RouteMode::Raindex(Some(id))) {
        let t = p.raindex.take(who, id).ok_or_else(|| routing::no(p, "Нужна новая котировка Raindex"))?;
        let result = super::super::raindex::settlement::trade(p, req.clone(), &t).await;
        (result, t.fallback_fee)
    } else if mode == Some(RouteMode::Rfq(id)) {
        if req.route.as_ref().and_then(|r|r.exec_note.as_deref())==Some(super::super::dexalot::LABEL) {
            let ticket=p.dexalot.take(who,id).ok_or_else(||routing::no(p,"Требуется новая подтверждённая заявка Dexalot"))?;
            return Box::pin(super::super::dexalot::trade(p,req,&ticket)).await;
        }
        if req.route.as_ref().and_then(|r|r.exec_note.as_deref())==Some(super::super::tokenlon::LABEL) {
            let ticket=p.tokenlon.take(who,id).ok_or_else(||routing::no(p,"Требуется новая персональная заявка Tokenlon"))?;
            return Box::pin(super::super::tokenlon::settlement::trade(p,req,&ticket)).await;
        }
        if req.route.as_ref().and_then(|r|r.exec_note.as_deref())==Some(super::super::airswap::LABEL) {
            let ticket=p.airswap.take(who,id).ok_or_else(||routing::no(p,"Требуется новая подтверждённая заявка AirSwap"))?;
            return Box::pin(super::super::airswap::trade(p,req,&ticket)).await;
        }
        if req.route.as_ref().and_then(|r|r.exec_note.as_deref())==Some(super::super::textile::LABEL) {
            let ticket=p.textile.take(who,id).ok_or_else(||routing::no(p,"Требуется новая подтверждённая заявка Textile"))?;
            return Box::pin(super::super::textile::trade(p,req,&ticket)).await;
        }
        let old=p.rfqs.take(who,id);
        let native=p.native_quotes.take(who,id);
        let gladius=p.gladius.take(who,id);
        let epsilon=p.epsilon.take(who,id);
        match (old,native,gladius,epsilon) {
            (None,None,None,Some(t))=>(super::super::epsilon::trade::trade(p,req.clone(),&t).await,t.fallback_fee),
            (None,None,Some(t),None)=>(super::super::gladius::settlement::trade(p,req.clone(),&t).await,t.fallback_fee),
            (Some(q),None,None,None)=>(selected(p,req.clone(),id,who,&q).await,q.fallback_fee),
            (None,Some(t),None,None)=>(super::super::native_rfq::routing::trade(p,req.clone(),&t).await,t.fallback_fee),
            _=>return Err(routing::no(p,"Подписанная RFQ отсутствует или неоднозначна, нужна новая котировка")),
        }
    } else {
        return Err(routing::no(p, "Билет не соответствует выбранному маршруту"));
    };
    let Err(e) = result else {
        return result;
    };
    if e.tx.is_some()
        || e.pending.is_some()
        || !matches!(e.code, ErrorCode::NoRoute | ErrorCode::QuoteMismatch)
    {
        return Err(e);
    }
    // По этому билету никто другой не может войти: take уже убрал единственную копию для исполнения.
    // До смены пути проверяем устойчивый маркер, даже если отказ пришёл из проверки после резервирования.
    if let Some(original) = prior(p, &req).await? {
        return Ok(original);
    }
    let Some(planned_fee) = fallback_fee else {
        return Err(e);
    };
    let quoting = p.quoting();
    let fresh = match quoting.quote(&req.asset, req.side, req.qty).await {
        Ok(v) => v,
        Err(_) => return Err(e),
    };
    if fresh.fillable_qty != req.qty
        || !fresh.route.as_ref().is_some_and(|r| r.executable)
        || super::super::best_of::choose::within_worst(&fresh, req.worst_price).is_err()
    {
        return Err(e);
    }
    let Some(net) = routing::total(p, &fresh).await else {
        return Err(e);
    };
    let Some(limit) = req.worst_price.checked_mul(req.qty) else {
        return Err(e);
    };
    let ok = match req.side {
        Side::Buy => limit.checked_add(planned_fee).is_some_and(|max| net <= max),
        Side::Sell => limit.checked_sub(planned_fee).is_some_and(|min| net >= min),
    };
    if !ok {
        return Err(e);
    }
    let mut fallback = req;
    fallback.route = fresh.route;
    Box::pin(exec::trade::trade(&quoting, &p.env(), &p.nonces, fallback)).await
}

async fn selected(
    p: &Parts,
    req: TradeRequest,
    id: B256,
    who: Address,
    q: &super::Prepared,
) -> GatewayResult<TradeResult> {
    let ctx = &p.ctx;
    if !super::enabled(ctx)
        || !super::api_wallet(ctx,&q.source).is_ok_and(|w|super::allowed(w,who))
        || ctx.text("hashflow.source")? != q.source
        || !routing::timely(p, q)
        || req.remeasure.as_ref().is_some_and(|r| !r.any_executor)
        || req.venue != ctx.venue()
        || req.qty <= Decimal::ZERO
        || req.worst_price <= Decimal::ZERO
    {
        return Err(routing::no(
            p,
            "RFQ больше не соответствует разрешённой сделке",
        ));
    }
    let chain = super::super::node::chain_id::chain_id(ctx)?;
    let coin = req
        .asset
        .contract
        .ok_or_else(|| routing::no(p, "У монеты отсутствует контракт"))?;
    let quote = p.tokens.usdt(ctx)?;
    let buy = req.side == Side::Buy;
    let (tokens, decs, coins, quoted) = if buy {
        ((quote, coin), (q.dout, q.di), q.output, q.input)
    } else {
        ((coin, quote), (q.di, q.dout), q.input, q.output)
    };
    if tokens != (q.request.input, q.request.output)
        || who != q.request.trader
        || super::super::num::to_decimal(coins, decs.0) != Some(req.qty)
    {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            "Кошелёк, токены или количество расходятся с выбранной RFQ",
        ));
    }
    let actual = super::super::num::to_decimal(quoted, decs.1)
        .ok_or_else(|| routing::no(p, "Сумма RFQ вне диапазона"))?;
    let limit = req
        .worst_price
        .checked_mul(req.qty)
        .ok_or_else(|| routing::no(p, "Лимит сделки вне диапазона"))?;
    if (buy && actual > limit)
        || (!buy && actual < limit)
        || (!buy && req.remeasure.as_ref().is_some_and(|r| actual < r.floor))
    {
        return Err(ctx
            .err(
                ErrorCode::QuoteMismatch,
                "Подписанная цена Hashflow хуже границы сделки",
            )
            .price_moved());
    }
    let env = p.env();
    let notice = |text: String| {
        env.stats.emit(MarketEvent::Notice {
            venue: ctx.venue(),
            code: "dex.swap_waits_inflow".into(),
            text,
        })
    };
    let hold = env.inflows.swap(ctx, tokens.1, &notice).await?;
    let slot = p.nonces.acquire(ctx).await?;
    if slot.from != who || !routing::timely(p, q) {
        return Err(routing::no(
            p,
            "Срок RFQ истёк в очереди кошелька или сменился отправитель",
        ));
    }
    let h = sim::header(ctx, chain)
        .await
        .map_err(|e| routing::no(p, &e))?;
    wire::verify(
        &q.firm,
        chain,
        super::router(chain).map_err(|e| routing::no(p, &e))?,
        &q.request,
        q.input,
        h.timestamp.to::<u64>(),
    )
    .map_err(|e| routing::no(p, &e))?;
    sim::run(ctx, h.hash, &q.request, &q.firm, q.input, q.output, q.value)
        .await
        .map_err(|e| routing::no(p, &e))?;
    let mut parse = exec::swap_ctx::SwapCtx::swap(who, tokens, decs, &[], None, None);
    let proof = ReceiptProof {
        original:Some(proof_chain::Original{chain,target:q.firm.target_contract,calldata:alloy_primitives::keccak256(&q.firm.calldata)}),
        pool: q.firm.quote_data.pool,
        trader: who,
        input: tokens.0,
        output: tokens.1,
        txid: id,
        amount: q.input,
        proceeds: q.output,
    };
    parse.rfq = Some(proof.clone());
    parse.who = Some("Hashflow RFQT".into());
    let dispatch = Dispatch {
        key: key(p, &req)?,
        expiry: q.preview.expires,
        gate: Gate::Hashflow(q.source.clone()),
        parse: parse.clone(),
    };
    let send = exec::txs::send(
        &env,
        &slot,
        q.firm.target_contract,
        q.firm.calldata.clone(),
        q.value,
        false,
        Some(hold),
    );
    let done=super::dispatch::submit(p,&req,dispatch,send).await?;
    if matches!(done.chain.outcome,TxOutcome::Included|TxOutcome::Reverted){
        let r=done.receipt.as_ref().ok_or_else(|| parse.attach(ctx.err(ErrorCode::OutcomeUnknown,"Отсутствует квитанция Hashflow").with_tx(done.chain.clone())))?;
        proof.included(ctx,done.chain.hash,done.chain.nonce,r).await.map_err(|e|parse.attach(e.with_tx(done.chain.clone())))?;
    }
    exec::result::by(
        exec::result::result(ctx, req, done, who, tokens, decs, &[]).map_err(|e| parse.attach(e)),
        "Hashflow RFQT",
    )
}
