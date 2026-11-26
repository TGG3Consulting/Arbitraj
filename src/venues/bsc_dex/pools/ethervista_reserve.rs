//! Резерв ETH адаптера EtherVista. Кнопка проверки и подготовка сделки читают одинаковые роли на одном блоке.
use super::{
    direct_module,
    ethervista_registry::{self, Deployment, IVista},
    families::{self, Family},
    family_variant::Variant,
    nami_batch as batch,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, num},
};
use alloy_primitives::{Address, U256};
use serde_json::{Value, json};

pub(super) async fn read(
    ctx: &Ctx,
    f: &Family,
    market: Deployment,
    tag: &Value,
) -> GatewayResult<(Address, Address, U256)> {
    let module = ctx.dex_addr("dex.router", f.id)?;
    let ([factory, router, wrapped, owner, accounted], balance) = tokio::try_join!(
        batch::read(
            ctx,
            [
                batch::view(tag, module, IVista::factoryCall {}),
                batch::view(tag, module, IVista::routerCall {}),
                batch::view(tag, module, IVista::wethCall {}),
                batch::view(tag, module, IVista::ownerCall {}),
                batch::view(tag, module, IVista::feeReserveCall {}),
            ]
        ),
        crate::venues::bsc_dex::node::rpc::call::<U256>(
            ctx,
            "eth_getBalance",
            json!([module, tag])
        )
    )?;
    let available = batch::decode(&accounted, IVista::feeReserveCall {})?;
    let reserve_owner = batch::decode(&owner, IVista::ownerCall {})?;
    if batch::decode(&factory, IVista::factoryCall {})? != market.factory
        || batch::decode(&router, IVista::routerCall {})? != market.router
        || batch::decode(&wrapped, IVista::wethCall {})? != market.wrapped
        || reserve_owner.is_zero()
        || balance < available
    {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "EtherVista: привязка или ETH-резерв модуля не подтверждены",
        ));
    }
    direct_module::verify_code(ctx, f.id, module, tag).await?;
    Ok((module, reserve_owner, available))
}

pub(in crate::venues::bsc_dex) async fn connection(ctx: &Ctx) -> Vec<String> {
    if !matches!(ctx.chain_id(), Some(1 | 8453)) {
        return Vec::new();
    }
    let families = families::all(ctx);
    let mut details = Vec::new();
    for f in families
        .into_iter()
        .filter(|f| f.variant == Variant::EtherVista)
    {
        let generation = ctx.settings.generation();
        let module = ctx.dex_addr("dex.router", f.id);
        if module
            .as_ref()
            .is_err_and(|e| e.code == ErrorCode::NotConfigured)
        {
            details.push(format!("{}: адрес адаптера не задан; резерв V5 для продаж не подключён. Прямой обмен использует возможности кошелька и разрешённого роутера.",f.id));
            continue;
        }
        let check=async {
            module?;
            let head=super::super::quote::white_state::head(ctx,"latest").await?;
            let tag=head.selector();
            let market=ethervista_registry::deployed(ctx,f,&tag).await?;
            let (addr,owner,available)=read(ctx,f,market,&tag).await?;
            if ctx.settings.generation()!=generation {
                return Err(ctx.err(ErrorCode::StaleData,"EtherVista: параметры изменились во время проверки"));
            }
            let eth=num::to_decimal(available,num::BNB_DECIMALS)
                .ok_or_else(||ctx.err(ErrorCode::BadResponse,"EtherVista: ETH-резерв не помещается в число"))?;
            Ok(format!("{}: адаптер {addr:#x}, владелец резерва {owner:#x}, резерв {} ETH. Продажа через V5 требует покрытия платы выбранного пула; покупка использует ETH из суммы входа.",f.id,eth.normalize()))
        }.await;
        details.push(
            check.unwrap_or_else(|e| format!("{}: резерв не подтверждён: {}", f.id, e.message)),
        );
    }
    details
}
