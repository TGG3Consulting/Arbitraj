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
    vista_pricing::is_vista(e) || matches!(e.family.variant,Variant::Pendle|Variant::BancorLegacy|Variant::YieldBasis)
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
        if e.family.variant == crate::venues::bsc_dex::pools::family_variant::Variant::BancorLegacy {
            Box::pin(super::bancor_legacy_plan::check_step(ctx,e,input,&head.selector())).await?;
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
