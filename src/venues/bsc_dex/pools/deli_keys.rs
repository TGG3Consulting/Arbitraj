//! Прообразы PoolId Deli из настроек и из Initialize. Событие сохраняет только ключ;
//! проверка реестра, ролей и выплат выполняется при последующем чтении.
use super::{abi_check::IPositionManagerKeys, deli::invalid, families::Family, key::{PoolKey,UniswapPoolKeyData}};
use crate::gateway::errors::GatewayResult;
use crate::venues::bsc_dex::ctx::Ctx;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address,B256,Bytes,keccak256};

pub(super) fn key(bytes:&[u8])->Option<PoolKey> {
    if bytes.len()!=160 {return None}
    let k=IPositionManagerKeys::poolKeysCall::abi_decode_returns_validate(bytes).ok()?;
    let fee:u32=k.fee.to();
    if k.currency0.is_zero() || k.currency0>=k.currency1 || k.hooks.is_zero() ||
        fee>=1_000_000 || k.tickSpacing.as_i32()<=0 {return None}
    Some(PoolKey::Uniswap(UniswapPoolKeyData{currency0:k.currency0,currency1:k.currency1,
        hooks:k.hooks,fee,tick_spacing:k.tickSpacing}))
}

pub(super) fn configured(ctx:&Ctx,f:&Family,id:B256)->GatewayResult<PoolKey> {
    let keys=match ctx.dex_list("dex.deli_pool_keys",f.id) {
        Ok(keys)=>keys, Err(e) if e.code==crate::gateway::errors::ErrorCode::NotConfigured=>Vec::new(), Err(e)=>return Err(e),
    };
    let mut selected=None;
    for encoded in keys {
        let raw:Bytes=encoded.parse().map_err(|_| invalid(ctx))?;
        if keccak256(&raw)==id {
            if selected.is_some() {return Err(invalid(ctx));}
            selected=key(&raw);
            if selected.is_none(){return Err(invalid(ctx));}
        }
    }
    // Здесь только прообраз хеша: право использовать рынок отдельно проверяет market на текущем блоке.
    selected.or_else(|| OBSERVED.read().ok()?.get(&id).cloned()).ok_or_else(||invalid(ctx))
}

pub(super) fn bindings(ctx:&Ctx,f:&Family)->GatewayResult<[(Address,B256);4]> {
    let roles=ctx.dex_list("dex.deli_role_bindings",f.id)?;
    let mut result=[(Address::ZERO,B256::ZERO);4];
    if roles.len()!=result.len(){return Err(invalid(ctx));}
    for (i,item) in roles.iter().enumerate(){
        let (a,h)=item.split_once('@').ok_or_else(||invalid(ctx))?;
        let a:Address=a.parse().map_err(|_|invalid(ctx))?;
        let h:B256=h.parse().map_err(|_|invalid(ctx))?;
        if a.is_zero() || h.is_zero() || h==keccak256([]) || result[..i].iter().any(|r|r.0==a) {
            return Err(invalid(ctx));
        }
        result[i]=(a,h);
    }
    Ok(result)
}

struct Claimed {
    store: crate::settings::store::Store,
    generation: u64,
    venue: &'static str,
    hooks: Vec<Address>,
}
thread_local! {
    static CLAIMED: std::cell::RefCell<Option<Claimed>>=const { std::cell::RefCell::new(None) };
}
pub(super) fn claimed(ctx:&Ctx,hook:Address)->bool {
    if hook.is_zero(){return false;}
    CLAIMED.with_borrow_mut(|slot|{
        let generation=ctx.settings.generation();
        if let Some(row)=slot && row.generation==generation && row.store.same(&ctx.settings) && row.venue==ctx.place.id {
            return row.hooks.contains(&hook);
        }
        let hooks:Vec<_>=super::families::all(ctx).into_iter().filter(|f|f.variant==super::family_variant::Variant::DeliConstant)
            .filter_map(|f| bindings(ctx,f).ok().map(|roles|roles[0].0)).collect();
        let contains=hooks.contains(&hook);
        *slot=Some(Claimed{store:ctx.settings.clone(),generation,venue:ctx.place.id,hooks});
        contains
    })
}

/// Сохраняет неизменяемый прообраз PoolId. Повтор в другой ветви с тем же хешем не может
/// изменить ключ. Доказательство инициализации и прав хука всегда читается заново.
static OBSERVED: std::sync::LazyLock<std::sync::RwLock<std::collections::HashMap<B256,PoolKey>>> =
    std::sync::LazyLock::new(Default::default);

pub(super) fn observed(ctx:&Ctx,f:&Family,manager:Address,pk:&PoolKey)->GatewayResult<bool> {
    if ctx.dex_addr("dex.pool_manager",f.id)?!=manager || bindings(ctx,f)?[0].0!=pk.hooks() {
        return Ok(false);
    }
    let Some(valid)=key(&pk.abi()) else{return Ok(false);};
    OBSERVED.write().map_err(|_| invalid(ctx))?.entry(valid.id()).or_insert(valid);
    Ok(true)
}
