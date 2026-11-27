//! Штатный Slipstream UniversalRouter выбирает клон по фабрике, реализации и tickSpacing.
use super::{abi_slipstream::ICLFactory, exact_wallet_tokens, nami_batch as batch, registry::Entry, verify_crypto_v1};
use crate::{gateway::errors::{ErrorCode,GatewayResult}, venues::bsc_dex::ctx::Ctx};
use alloy_primitives::{Address,B256,keccak256,aliases::I24};
use serde_json::{Value,json};

pub(crate) async fn checked(ctx:&Ctx,e:&Entry,input:Address,output:Address,at:&Value)->GatewayResult<Address>{
    let id=e.family.id;
    let generation=ctx.settings.generation();
    let bad=||ctx.err(ErrorCode::NoRoute,"Slipstream: изменились фабрика, реализация, Permit2 или проверенный код маршрута");
    let factory=ctx.dex_addr("dex.factory",id)?;
    let permit=ctx.dex_addr("dex.permit2",id)?;
    let router=ctx.dex_addr("dex.universal_router",id)?;
    let pool=e.pool.reference.address();
    let (a,b)=(e.pool.token0,e.pool.token1);
    if !((input,output)==(a,b) || (input,output)==(b,a)) || a>=b{return Err(bad())}
    let spacing=I24::try_from(e.pool.tick_spacing.filter(|s|*s>0).ok_or_else(bad)?).map_err(|_|bad())?;
    let lookup=ICLFactory::getPoolCall {tokenA:a,tokenB:b,tickSpacing:spacing};
    let hash=|key|ctx.dex_text(key,id)?.parse::<B256>().map_err(|_|bad());
    let (rh,ph)=(hash("dex.router_runtime_hash")?,hash("dex.permit2_runtime_hash")?);
    let ([code,pcode,clone,registered],())=tokio::try_join!(batch::read(ctx,[
        ("eth_getCode",json!([router,at])),("eth_getCode",json!([permit,at])),
        ("eth_getCode",json!([pool,at])),batch::view(at,factory,lookup.clone())]),
        exact_wallet_tokens::check(ctx,id,input,output,at))?;
    let implementation=verify_crypto_v1::proxy_target(&clone).ok_or_else(bad)?;
    // Положения слов конструктора в проверенном коде UniversalRouter. Полный runtime включает значения.
    let bound=|i:usize,address:Address|code.get(i..i+32)==Some(address.into_word().as_slice());
    if code.is_empty() || pcode.is_empty() || keccak256(&code)!=rh || keccak256(&pcode)!=ph
        || !bound(19561,factory) || !bound(19480,implementation)
        || ![5275,5531,8694,10012,15665].into_iter().all(|i|bound(i,permit))
        || batch::decode(&registered,lookup)?!=pool || generation!=ctx.settings.generation(){return Err(bad())}
    Ok(router)
}
