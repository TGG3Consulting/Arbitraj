//! Проверка роли и покрытия EtherVista по фактическому направлению каждого шага V5.
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        pools::registry::Entry,
        quote::{vista_pricing, white_state},
    },
};
use alloy_primitives::Address;

pub(super) fn needs(e:&Entry)->bool{
    use crate::venues::bsc_dex::pools::family_variant::Variant;
    e.family.variant.aqua() || e.family.variant.llamma() || e.family.variant.ammalgam() || vista_pricing::is_vista(e) || e.family.variant.ambient() || e.family.variant.fluid_lite() || e.family.variant.motoswap() || matches!(e.family.variant,Variant::Pendle|Variant::Brokerbot|Variant::BancorLegacy|Variant::Mooniswap|Variant::YieldBasis)
}
pub(super) async fn check(
    ctx: &Ctx,
    steps: Vec<(&Entry, Address)>,
    generation: u64,
) -> GatewayResult<()> {
    if steps.is_empty() {
        return Ok(());
    }
    let head = white_state::head(ctx, "latest").await?;
    let mut markets: Vec<(
        &str,
        crate::core::types::PoolRef,
        crate::venues::bsc_dex::pools::ethervista::VistaMarket,
    )> = Vec::new();
    for (e, input) in steps {
        if e.family.variant.fluid_lite(){Box::pin(super::fluid_lite_plan::graph(ctx,e,input,&head.selector())).await?;continue;}
        if e.family.variant.ambient(){Box::pin(super::ambient_plan::graph(ctx,e,input,&head.selector())).await?;continue;}
        if e.family.variant.llamma(){Box::pin(super::super::pools::llamma::check(ctx,e,&head.selector())).await?;continue;}
        if e.family.variant.ammalgam() {
            Box::pin(super::super::pools::ammalgam::check_step(ctx,e,input,&head.selector())).await?;
            continue;
        }
        if e.family.variant.issuer_shares() {
            let m=super::aktionariat_plan::terms(ctx,e)?;
            Box::pin(super::aktionariat_plan::verify(ctx,e,m,&head.selector())).await?;
            continue;
        }
        if e.family.variant == crate::venues::bsc_dex::pools::family_variant::Variant::BancorLegacy {
            Box::pin(super::bancor_legacy_plan::check_step(ctx,e,input,&head.selector())).await?;
            continue;
        }
        if e.family.variant.aqua(){
            let v5=super::executor::address(ctx)?.ok_or_else(||crate::venues::bsc_dex::pools::aqua::bad(ctx))?;
            Box::pin(super::aqua_plan::cached(ctx,e,head,v5)).await?;continue;
        }
        if e.family.variant.motoswap() {
            let (_,target)=super::motoswap_plan::graph_target(ctx,e).ok_or_else(||super::motoswap_plan::bad(ctx))?;
            let at=head.selector();
            tokio::try_join!(super::motoswap_plan::terms(ctx,e,input,target,Some(&at)),
                super::super::pools::motoswap::state(ctx,e.family,e.pool.reference.address(),(e.pool.token0,e.pool.token1),&at))?;
            continue;
        }
        if e.family.variant.mooniswap() {
            Box::pin(super::mooniswap_plan::check_step(ctx,e,input,&head.selector())).await?;
            continue;
        }
        if e.family.variant == crate::venues::bsc_dex::pools::family_variant::Variant::YieldBasis {
            Box::pin(super::yield_basis_plan::check_step(ctx,e,input,&head.selector())).await?;
            continue;
        }
        if e.family.variant==crate::venues::bsc_dex::pools::family_variant::Variant::Pendle {
            Box::pin(crate::venues::bsc_dex::pools::pendle_module::checked(ctx,e,input,head)).await?;continue
        }
        if !vista_pricing::is_vista(e) {
            continue;
        }
        let index = markets
            .iter()
            .position(|(id, p, _)| *id == e.family.id && p == &e.pool.reference);
        let market = if let Some(i) = index {
            &markets[i].2
        } else {
            let value =
                vista_pricing::market(ctx, e, head, super::vista_wire::Form::ModuleToken).await?;
            markets.push((e.family.id, e.pool.reference.clone(), value));
            &markets.last().expect("вставлен рынок").2
        };
        if market.reserve_owner.is_none() {
            return Err(ctx.err(
                ErrorCode::NotConfigured,
                "EtherVista: владелец резерва модуля не подтверждён",
            ));
        }
        vista_pricing::wire(ctx, e, input, market, super::vista_wire::Form::ModuleToken)?;
    }
    if ctx.settings.generation() != generation {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Настройки модуля изменились после подготовки исполнения",
        ));
    }
    Ok(())
}
