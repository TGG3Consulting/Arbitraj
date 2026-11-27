//! Прообразы стратегий и наблюдавшиеся валюты. Доступность проверяется rawBalances в момент чтения.
use super::aqua::{self, IAqua, Program, Roles};
use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address,B256,Bytes};
use serde::{Deserialize,Serialize};
use std::collections::{BTreeMap,BTreeSet,HashMap};
use std::sync::{Arc,LazyLock,RwLock};

#[derive(Clone,Default,Serialize,Deserialize)]
pub(super) struct Book {
    rows:BTreeMap<B256,Record>,
}
#[derive(Clone,Serialize,Deserialize)]
struct Record {
    maker:Address,
    raw:Bytes,
    assets:BTreeSet<Address>,
}
#[derive(Clone)]
pub(in crate::venues::bsc_dex) struct Offer {
    pub program:Program,
    pub hash:B256,
    pub first:bool,
}
impl Book {
    pub(super) fn insert(&mut self,maker:Address,h:B256,raw:Bytes)->bool {
        if self.rows.contains_key(&h){return self.rows[&h].maker==maker;}
        if aqua::program(maker,h,&raw).is_none(){return false;}
        self.rows.insert(h,Record{maker,raw,assets:BTreeSet::new()});
        true
    }
    pub(super) fn tokens(&mut self,maker:Address,h:B256,tokens:&[Address]){
        if let Some(r)=self.rows.get_mut(&h).filter(|r|r.maker==maker){
            r.assets.extend(tokens.iter().copied().filter(|t|!t.is_zero()));
        }
    }
    pub(super) fn observe(&mut self,roles:Roles,log:&Log)->Option<B256>{
        let emitter=log.inner.address;
        if log.removed{return None;}
        let topic=log.topics().first().copied().unwrap_or_default();
        if emitter==roles.registry{
            if topic==IAqua::Shipped::SIGNATURE_HASH && let Ok(e)=IAqua::Shipped::decode_log_data(&log.inner.data){
                if e.app==roles.engine && self.insert(e.maker,e.strategyHash,e.strategy){return Some(e.strategyHash);}
            }else if topic==IAqua::Pushed::SIGNATURE_HASH && let Ok(e)=IAqua::Pushed::decode_log_data(&log.inner.data){
                if e.app==roles.engine {self.tokens(e.maker,e.strategyHash,&[e.token]);return Some(e.strategyHash);}
            }
            // Docked не перечисляет токены. Сохранённый прообраз по этому событию не уничтожается.
        }else if emitter==roles.engine && topic==IAqua::Swapped::SIGNATURE_HASH && let Ok(e)=IAqua::Swapped::decode_log_data(&log.inner.data){
            self.tokens(e.maker,e.orderHash,&[e.tokenIn,e.tokenOut]);return Some(e.orderHash);
        }
        None
    }
    pub(super) fn hashes(&self)->impl Iterator<Item=B256>+'_ {self.rows.keys().copied()}
    pub(super) fn assets(&self,h:B256)->Option<&BTreeSet<Address>>{self.rows.get(&h).map(|r|&r.assets)}
    pub(super) fn pairs(&self,pair:(Address,Address))->impl Iterator<Item=(B256,bool)>+'_ {
        self.rows.iter().filter(move|(_,r)|r.assets.contains(&pair.0)&&r.assets.contains(&pair.1))
            .flat_map(|(h,_)|[(*h,false),(*h,true)])
    }
    pub(super) fn offer(&self,id:B256,pair:(Address,Address))->Option<Offer>{
        let (h,first)=self.pairs(pair).find(|(h,first)|aqua::edge(*h,pair,*first)==id)?;
        let r=self.rows.get(&h)?;
        Some(Offer{program:aqua::program(r.maker,h,&r.raw)?,hash:h,first})
    }
}
type Namespace=(u64,Address,Address);
static BOOKS:LazyLock<RwLock<HashMap<Namespace,Arc<Book>>>>=LazyLock::new(Default::default);
pub(super) fn current(chain:u64,r:Roles)->Arc<Book>{
    BOOKS.read().unwrap_or_else(|e|e.into_inner()).get(&(chain,r.registry,r.engine)).cloned().unwrap_or_default()
}
pub(super) fn publish(chain:u64,r:Roles,b:Book){
    BOOKS.write().unwrap_or_else(|e|e.into_inner()).insert((chain,r.registry,r.engine),Arc::new(b));
}

pub(super) fn observed(chain:u64, roles:Roles, log:&Log)->Option<B256>{
    let mut books=BOOKS.write().unwrap_or_else(|e|e.into_inner());
    let b=books.entry((chain,roles.registry,roles.engine)).or_default();
    Arc::make_mut(b).observe(roles,log)
}
