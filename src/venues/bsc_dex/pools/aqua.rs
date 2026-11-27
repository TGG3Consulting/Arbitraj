//! Запись Aqua определяется владельцем, приложением и хешем полных ABI-байтов.
//! Переводы мейкера могут финансироваться его хуками во время обмена.
//! https://business.1inch.com/portal/documentation/aqua/reference/events-and-interfaces.md
use super::{bancor_legacy::read, families::Family, safeswap_trade::hash, solidly_classic_factory::plain};
use crate::{gateway::errors::{ErrorCode,GatewayResult},venues::bsc_dex::ctx::Ctx};
use alloy::sol_types::SolValue;
use alloy_primitives::{Address,B256,Bytes,U256,keccak256};
use serde_json::Value;

alloy::sol! {
    #[derive(Debug,PartialEq,Eq)]
    struct Program { address maker; uint256 traits; bytes data; }
    interface IAqua {
        event Shipped(address maker,address app,bytes32 strategyHash,bytes strategy);
        event Docked(address maker,address app,bytes32 strategyHash);
        event Pushed(address maker,address app,bytes32 strategyHash,address token,uint256 amount);
        event Pulled(address maker,address app,bytes32 strategyHash,address token,uint256 amount);
        event Swapped(bytes32 orderHash,address maker,address taker,address tokenIn,address tokenOut,uint256 amountIn,uint256 amountOut);
        function AQUA() external view returns(address);
        function rawBalances(address maker,address app,bytes32 orderHash,address token) external view returns(uint248 amount,uint8 count);
        function quote(Program order,address tokenIn,address tokenOut,uint256 amount,bytes taker) external view returns(uint256 paid,uint256 net,bytes32 h);
    }
}

pub(in crate::venues::bsc_dex) fn bad(ctx:&Ctx)->crate::gateway::errors::GatewayError {
    ctx.err(ErrorCode::NotApplicable,"Aqua: не подтверждены программа, регистрация или исполняющее приложение")
}

pub(in crate::venues::bsc_dex) fn program(maker:Address,h:B256,raw:&[u8])->Option<Program> {
    let p=Program::abi_decode_validate(raw).ok()?;
    (!maker.is_zero() && p.maker==maker && p.traits.bit(254) && keccak256(raw)==h && p.abi_encode()==raw).then_some(p)
}

/// Разные пары многоактивной программы и два проверяемых порядка привязаны к разным рёбрам.
pub(super) fn edge(h:B256,pair:(Address,Address),first:bool)->B256 {
    keccak256((h,pair.0,pair.1,first).abi_encode_params())
}

#[derive(Clone,Copy)]
pub(in crate::venues::bsc_dex) struct Roles {pub registry:Address,pub engine:Address}
impl Roles {
    pub async fn checked(ctx:&Ctx,f:&Family,at:&Value)->GatewayResult<Self> {
        let registry=ctx.dex_addr("dex.factory",f.id)?;
        let engine=ctx.dex_addr("dex.quoter",f.id)?;
        let reg_hash=hash(ctx,f.id,"dex.factory_runtime_hash")?;
        let vm_hash=hash(ctx,f.id,"dex.quoter_runtime_hash")?;
        let (_,_,points)=tokio::try_join!(plain(ctx,registry,reg_hash,at),plain(ctx,engine,vm_hash,at),read(ctx,engine,IAqua::AQUACall{},at))?;
        if registry==engine || points!=registry{return Err(bad(ctx));}
        Ok(Self{registry,engine})
    }
    pub async fn active(self,ctx:&Ctx,p:&Program,h:B256,pair:(Address,Address),at:&Value)->GatewayResult<()> {
        let one=|token|read(ctx,self.registry,IAqua::rawBalancesCall{maker:p.maker,app:self.engine,orderHash:h,token},at);
        let (a,b)=tokio::try_join!(one(pair.0),one(pair.1))?;
        if [a.count,b.count].iter().any(|n| *n==0 || *n==u8::MAX) || pair.0>=pair.1 || pair.0.is_zero() {return Err(bad(ctx));}
        Ok(())
    }
}

/// SwapVM TakerTraits: десять конечных смещений uint16, затем flags и опциональные поля.
pub(in crate::venues::bsc_dex) fn traits(minimum:U256,recipient:Address,deadline:u64,first:bool)->Option<Bytes> {
    if deadline>=(1u64<<40) || recipient.is_zero(){return None;}
    let mut b=Vec::with_capacity(79);
    for n in [57u16,57,57,57,57,57,57,57,52,32]{b.extend(n.to_be_bytes());}
    b.extend(if first{0x61u16}else{0x41}.to_be_bytes());
    b.extend(minimum.to_be_bytes::<32>());b.extend_from_slice(recipient.as_slice());
    b.extend_from_slice(&deadline.to_be_bytes()[3..]);Some(b.into())
}

pub(super) fn topics()->[B256;4]{
    use alloy::sol_types::SolEvent;
    [IAqua::Shipped::SIGNATURE_HASH,IAqua::Docked::SIGNATURE_HASH,IAqua::Pushed::SIGNATURE_HASH,IAqua::Swapped::SIGNATURE_HASH]
}
