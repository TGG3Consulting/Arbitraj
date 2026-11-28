//! Персональные ставки SDK запрашиваются в фоне. Размер каждой ставки известен; линейного продолжения глубины нет.
use super::{client, profile, routing::Ticket};
use crate::{core::types::Side,gateway::events::{MarketEvent,SourceId},venues::bsc_dex::{parts::Parts,wallet}};
use alloy_primitives::{Address,B256};
use chrono::Utc;
use futures_util::{stream,StreamExt};
use rust_decimal::Decimal;
use std::{collections::{BTreeMap,BTreeSet,HashMap},sync::{Arc,Mutex},time::{Duration,Instant}};
use tokio::sync::mpsc;

#[derive(Clone,Eq,PartialEq,Hash)]
struct Key {wallet:String,base:Address,quote:Address,buy:bool,qty:Decimal}
struct Entry { generation:u64, at:Instant, rate:client::Rate }
#[derive(Default)]
struct State {requests:HashMap<(String,Address,Address,bool),Decimal>, rates:HashMap<Key,Entry>, tickets:HashMap<(Address,B256),Arc<Ticket>>}
#[derive(Default,Clone)]
pub(in crate::venues::bsc_dex) struct Cache(Arc<Mutex<State>>);

impl Cache {
    pub(super) fn rates(&self,p:&Parts,base:Address,quote:Address,side:Side)->Vec<(Decimal,client::Rate)> {
        let Ok(age)=p.ctx.int("feed.stale_after_ms") else{return vec![]};
        let buy=side==Side::Buy;
        let Ok(state)=self.0.lock() else{return vec![]};
        // Не привязанный к счёту предварительный расчёт видит лучшую наблюдавшуюся ставку.
        // Подтверждение и отправка проверяют исключительно ключ конкретного кошелька.
        let mut rates=BTreeMap::<Decimal,client::Rate>::new();
        let generation=p.ctx.settings.generation();
        for (key,entry) in &state.rates {
            if key.base==base && key.quote==quote && key.buy==buy && p.ctx.account.as_ref().is_none_or(|w|w==&key.wallet)
                && entry.generation==generation && entry.at.elapsed()<Duration::from_millis(age) {
                rates.entry(key.qty).and_modify(|r|{if entry.rate.price>r.price {*r=entry.rate}}).or_insert(entry.rate);
            }
        }
        rates.into_iter().collect()
    }
    /// Один дополнительный размер на кошелёк, пару и сторону за проход монитора.
    /// Количество должно попасть в границы свежего ответа именно этого кошелька.
    pub(super) fn request_size(&self,p:&Parts,base:Address,quote:Address,side:Side,qty:Decimal) {
        let Ok(age)=p.ctx.int("feed.stale_after_ms") else{return};
        let Some(dec)=p.tokens.decimals_known(base) else{return};
        if qty <= Decimal::ZERO || qty.normalize().scale()>u32::from(dec) {return;}
        let rev=p.ctx.settings.generation();
        let mut state=self.0.lock().unwrap_or_else(|e|e.into_inner());
        let buy=side==Side::Buy;
        let wallets:BTreeSet<_>=state.rates.iter().filter(|(k,e)|
            (k.base,k.quote,k.buy)==(base,quote,buy) && p.ctx.account.as_ref().is_none_or(|w|w==&k.wallet)
            && e.generation==rev && e.at.elapsed()<Duration::from_millis(age)
            && qty>=e.rate.min && qty<=e.rate.max).map(|(k,_)|k.wallet.clone()).collect();
        for wallet in wallets {state.requests.entry((wallet,base,quote,buy)).or_insert(qty);}
    }
    pub(super) fn save(&self,t:Ticket) {
        let mut s=self.0.lock().unwrap_or_else(|e|e.into_inner());
        let now=Utc::now().timestamp().max(0) as u64;
        s.tickets.retain(|_,v|v.proof.expiry>now);
        s.tickets.insert((t.proof.terms.payer,t.proof.tx_hash),Arc::new(t));
    }
    pub(in crate::venues::bsc_dex) fn take(&self,w:Address,id:B256)->Option<Arc<Ticket>> {
        self.0.lock().ok()?.tickets.remove(&(w,id))
    }
    fn event(&self,p:&Parts,cash:Address,age:u64)->MarketEvent {
        let rev=p.ctx.settings.generation();
        let mut markets=BTreeMap::new();
        let mut last=0;
        let now=Utc::now().timestamp_millis();
        if let Ok(s)=self.0.lock() {
            for (k,v) in &s.rates {
                let elapsed=v.at.elapsed().as_millis() as u64;
                if rev!=v.generation || k.quote!=cash || elapsed>=age {continue;}
                let entry=markets.entry(k.base).or_insert((false,false));
                if k.buy {entry.0=true;} else {entry.1=true;}
                last=last.max(now.saturating_add((age-elapsed).min(i64::MAX as u64) as i64));
            }
        }
        MarketEvent::RfqMarkets {source:SourceId::new(super::LABEL),venue:p.ctx.venue(),quote:cash,markets,expires_ms:last}
    }
}
pub(in crate::venues::bsc_dex) async fn clear(p:&Parts,out:&mpsc::Sender<MarketEvent>) {
    let had={
        let mut s=p.tokenlon.0.lock().unwrap_or_else(|e|e.into_inner());
        let had=!s.rates.is_empty();s.rates.clear();s.requests.clear();s.tickets.clear();had
    };
    if had {let _=out.send(p.tokenlon.event(p,Address::ZERO,0)).await;}
}

pub(in crate::venues::bsc_dex) async fn run(p:Parts,out:mpsc::Sender<MarketEvent>) {
    let mut settings=p.ctx.settings.subscribe();
    loop {
        let rev=p.ctx.settings.generation();
        let config=(super::enabled(&p.ctx) && super::private_node::configured(&p.ctx)).then(||{
            Some((profile::markets(&p.ctx)?,p.tokens.usdt(&p.ctx).ok()?,p.ctx.int("feed.stale_after_ms").ok()?.max(1)))
        }).flatten();
        let Some((markets,cash,age))=config else{
            clear(&p,&out).await;
            if matches!(settings.recv().await,Err(tokio::sync::broadcast::error::RecvError::Closed)){return;}
            continue;
        };
        let wallets:BTreeSet<_>=p.ctx.settings.accounts().rows().into_iter()
            .filter(|(row,_)|row!=crate::settings::accounts::ROW_PROBE).map(|(_,id)|id).collect();
        let clock=Instant::now();
        // Один клиент сериализует обращения каждого кошелька в своей STOMP-сессии.
        // Перемежаем кошельки в очереди: медленный аккаунт не задерживает публикацию остальных.
        let mut accounts=Vec::new();
        for id in wallets {
            if p.ctx.settings.generation()!=rev {break;}
            let ctx=p.ctx.with_account(&id);
            if let Ok(signer)=wallet::signer::signer(&ctx){accounts.push((id,ctx,Arc::new(signer)));}
        }
        let mut tasks=BTreeMap::new();
        let mut requests=std::mem::take(&mut p.tokenlon.0.lock().unwrap_or_else(|e|e.into_inner()).requests);
        for market in markets.iter().filter(|m|m.quote_token==cash){
            let Ok(base_dec)=p.tokens.decimals(&p.ctx,market.base_token).await else {continue};
            for (side,input,output) in [(Side::Buy,cash,market.base_token),(Side::Sell,market.base_token,cash)]{
                let buy=side==Side::Buy;
                // Перемежаем аккаунты и сохраняем настроенные наблюдения при новых запросах ядра.
                for size in 0..=market.sizes.len() {
                    for (id,c,signer) in &accounts{
                        let qty=match market.sizes.get(size) {
                            Some(qty)=>*qty,
                            None=>match requests.remove(&(id.clone(),market.base_token,cash,buy)) {
                                Some(qty) if !market.sizes.contains(&qty)=>qty,
                                _=>continue,
                            }
                        };
                        let k=Key{wallet:id.clone(),base:market.base_token,quote:cash,buy,qty};
                        let q=client::Query{base:market.base.clone(),quote:market.quote.clone(),side,amount:qty,base_dec,input,output};
                        tasks.entry(id.clone()).or_insert_with(Vec::new).push((k,q,c.clone(),Arc::clone(signer)));
                    }
                }
            }
        }
        if !tasks.is_empty(){let _=p.gas.fees(&p.ctx,&p.heads).await;}
        let completed=stream::iter(tasks.into_values()).flat_map_unordered(client::rate_parallelism(&p.ctx),|tasks|
            Box::pin(stream::iter(tasks).then(move |(key,query,ctx,signer)|async move{
            if ctx.settings.generation()!=rev || !super::enabled(&ctx) {return None;}
            let at=Instant::now();
            let rate=tokio::time::timeout(Duration::from_millis(age),client::rate(&ctx,&signer,&query)).await.ok()?.ok()?;
            (ctx.settings.generation()==rev && at.elapsed()<Duration::from_millis(age))
                .then_some((key,Entry{generation:rev,at,rate}))
        })));
        tokio::pin!(completed);
        while let Some(response)=completed.next().await{
            if p.ctx.settings.generation()!=rev{break;}
            if let Some((key,entry))=response{
                p.tokenlon.0.lock().unwrap_or_else(|e|e.into_inner()).rates.insert(key,entry);
                if out.send(p.tokenlon.event(&p,cash,age)).await.is_err(){return;}
            }
        }
        if let Ok(mut s)=p.tokenlon.0.lock() {
            s.rates.retain(|_,v|v.generation==rev && v.at.elapsed()<Duration::from_millis(age));
        }
        let delay=Duration::from_millis((age/2).max(1)).saturating_sub(clock.elapsed());
        tokio::select!{_=tokio::time::sleep(delay)=>{},change=settings.recv()=>if matches!(change,Err(tokio::sync::broadcast::error::RecvError::Closed)){return}}
    }
}
