//! PMMV5 переводит ERC20 без сравнения balanceOf получателя в контракте. В торговлю допускаем проверенные формы перевода.
use super::roles::Asset;
use crate::{gateway::errors::{ErrorCode,GatewayResult},venues::bsc_dex::{ctx::Ctx,node::rpc,pools::solidly_classic_factory as binding}};
use alloy_primitives::{Address,B256,Bytes,U256,b256,keccak256};
use serde_json::{Value,json};

async fn check(ctx:&Ctx,profile:&Asset,tag:&Value)->GatewayResult<()> {
    let bad=||ctx.err(ErrorCode::QuoteMismatch,"Tokenlon: реализация или условия удержания ERC20 не подтверждены");
    let token=profile.token.address;
    // Хеш выбирает доказанную семантику. Адрес размещения обязан совпадать с профилем кошелька.
    let hash=match profile.id.as_str() {
        "wrapped-native"=>b256!("d0a06b12ac47863b5c7be4185c2deaad1c61557033f56c7d4ea74429cbb25e23"),
        "exact-wbtc"=>b256!("131ff5c755b710d543ea70fede2eb38e5d15b1456df0ae932ba12e2786f7e5df"),
        "exact-uni"=>b256!("deba17f16fdba566b45d8019575e068625403cc6986fa17ceadd6edf08aa0868"),
        "tether-net"=>b256!("b44fb4e949d0f78f87f79ee46428f23a2a5713ce6fc6e0beb3dda78c2ac1ea55"),
        "fiat-usdc"=>b256!("d80d4b7c890cb9d6a4893e6b52bc34b56b25335cb13716e0d1d31383e6b41505"),
        _=>return Err(bad()),
    };
    if profile.token.runtime!=hash{return Err(bad());}
    binding::plain(ctx,token,hash,tag).await?;
    if profile.id=="tether-net" {
        let req=["basisPointsRate()","maximumFee()","deprecated()","upgradedAddress()","paused()"].map(|key|
            ("eth_call",json!([{"to":token,"data":Bytes::copy_from_slice(&keccak256(key.as_bytes())[..4])},tag])));
        let results=rpc::batch(ctx,&req).await?;
        if results.len()!=req.len() {return Err(bad());}
        for result in results {
            let bytes:Bytes=serde_json::from_str(result?.get()).map_err(|_|bad())?;
            if bytes.len()!=32 || U256::from_be_slice(&bytes)!=U256::ZERO {return Err(bad());}
        }
    }
    if profile.id=="fiat-usdc" {
        let implementation=profile.impl_role.as_ref().ok_or_else(bad)?;
        let slot=keccak256(b"org.zeppelinos.proxy.implementation");
        let value:B256=rpc::call(ctx,"eth_getStorageAt",json!([token,slot,tag])).await?;
        if value!=B256::left_padding_from(implementation.address.as_slice()) {return Err(bad());}
        if implementation.runtime!=b256!("cdfb7d322961af3acae7a8f7ee8b69c205b36f576cc5b077f170c7eb8ecbe3ea"){return Err(bad());}
        binding::plain(ctx,implementation.address,implementation.runtime,tag).await?;
    }
    Ok(())
}

pub(super) async fn exact_transfers(ctx:&Ctx,input:Address,output:Address,tag:&Value)->GatewayResult<()> {
    let bad=||ctx.err(ErrorCode::NotConfigured,"Для Tokenlon требуется подтверждённый профиль ERC20 обеих сторон");
    let assets=super::roles::assets(ctx).ok_or_else(bad)?;
    let i=assets.iter().find(|r|r.token.address==input).ok_or_else(bad)?;
    let o=assets.iter().find(|r|r.token.address==output).ok_or_else(bad)?;
    tokio::try_join!(check(ctx,i,tag),check(ctx,o,tag))?;
    Ok(())
}
