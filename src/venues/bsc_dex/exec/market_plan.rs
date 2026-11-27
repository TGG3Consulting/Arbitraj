//! Подготовка одной прямой ноги. Проба и денежная отправка используют одну и ту же реализацию площадки.
use super::super::{
    ctx::Ctx,
    pools::{family_variant::Variant, registry::Entry},
    quote::path::Hop,
    wallet::approve::Approval,
};
use super::{
    chain::Step,
    mint_club_wire::Protection,
    plan::{Order, Plan},
};
use crate::core::types::PoolState;
use crate::gateway::errors::GatewayResult;
use alloy_primitives::{Address, B256, Bytes};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::venues::bsc_dex) enum Market {
    Ranges,
    RegisteredV3,
    SolidlyRouter,
    Umbrae,
    O1,
    Antfarm,
    Dagrow,
    SafeSwap,
    Pendle,
    Bundles,
    Temple,
    Carbon,
    MintClub,
}

#[derive(Clone, Copy)]
pub(super) struct Selection {
    target: Option<Address>,
    carbon: Option<B256>,
    dagrow:Option<(Address,Option<alloy_primitives::U256>)>,
    proceeds: Option<(u16, bool)>,
    flux_eth:bool,
}
impl Selection {
    pub(super) fn for_leg(leg: &crate::core::types::RouteLeg) -> Self {
        if let [hop] = leg.hops.as_slice()
            && let Some(crate::core::types::RouterFeeMode::AntfarmProceeds { target, tier, loan }) =
                hop.fee_mode
        {
            return Self {
                target: Some(target),
                carbon: None,
                dagrow:None,
                proceeds: Some((tier, loan)),
                flux_eth:false,
            };
        }
        Self {
            target: super::mint_club_direct::selected(leg),
            carbon: super::carbon_direct::selected(leg).1,
            dagrow:match leg.hops.as_slice(){[h]=>match h.fee_mode{Some(crate::core::types::RouterFeeMode::Dagrow{target,desired})=>Some((target,desired)),_=>None},_=>None},
            proceeds: None,
            flux_eth:matches!(leg.hops.as_slice(),[hop] if matches!(hop.fee_mode,Some(crate::core::types::RouterFeeMode::FluxEth{..}))),
        }
    }
}
impl Market {
    pub(super) fn of(e: &Entry) -> Option<Self> {
        match &e.pool.state {
            _ if matches!(e.family.variant,Variant::SolidlyProxy|Variant::SolidlyPairOverride|Variant::V2ExactRouter) => Some(Self::SolidlyRouter),
            _ if e.family.variant==Variant::PuzzleRanges=>Some(Self::Ranges),
            _ if matches!(e.family.variant, Variant::RegisteredUniV3 | Variant::VvsV3) => Some(Self::RegisteredV3),
            _ if super::o1_wallet::enabled(e) => Some(Self::O1),
            _ if e.family.variant.umbrae() => Some(Self::Umbrae),
            PoolState::Antfarm(_) => Some(Self::Antfarm),
            PoolState::Bundles(_) => Some(Self::Bundles),
            _ if e.family.variant==Variant::SafeSwapTrade => Some(Self::SafeSwap),
            _ if e.family.variant==Variant::Pendle && e.family.executor_kind.is_some()=>Some(Self::Pendle),
            PoolState::Dagrow(_) => Some(Self::Dagrow),
            PoolState::Temple(_) if e.family.executor_kind.is_some() => Some(Self::Temple),
            PoolState::Carbon(_) => Some(Self::Carbon),
            PoolState::MintClub(_) => Some(Self::MintClub),
            _ => None,
        }
    }
    pub(super) fn dry(self, direct: bool) -> bool {
        direct || matches!(self, Self::Carbon | Self::MintClub)
    }
    pub(super) fn currency(self, e: &Entry, native: bool) -> bool {
        match (&e.pool.state, self) {
            (_,Self::O1) if e.family.variant.flux_native()=>true,
            (PoolState::Carbon(s), Self::Carbon) => s.native_side.is_some() == native,
            _ => !native,
        }
    }
    pub(super) fn missing_selection(self) -> &'static str {
        match self {
            Self::SolidlyRouter => "Solidly Router01: требуется исходный адрес исполнения",
            Self::Ranges=>"Ranges: требуется исходный адрес исполнения",
            Self::RegisteredV3 => "V3 Router: требуется исходный адрес исполнения",
            Self::O1 => "o1: выберите проверенный модуль для расчёта WETH",
            Self::Umbrae => "Umbrae: требуется выбранный адрес исполнения",
            Self::Antfarm => "Antfarm: требуется подтверждённый бюджет комиссии",
            Self::Bundles=>"Bundles Swap: требуется выбранный адрес исполнения",
            Self::Dagrow => "DAGrow: требуются исходные условия поручения",
            Self::SafeSwap => "SafeSwap: требуется выбранный модуль с оплатой из WETH",
            Self::Pendle => "Pendle: требуется выбранный модуль этой серии",
            Self::Temple => "TempleDAO: требуется выбранный модуль денежного маршрута",
            Self::Carbon => "Carbon: требуется подтвержденный вектор действий контроллера",
            Self::MintClub => {
                "Mint Club: требуется прямое поручение с собственными границами суммы"
            }
        }
    }
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::SolidlyRouter => "Solidly Router01",
            Self::Ranges=>"Ranges",
            Self::RegisteredV3 => "V3 Router",
            Self::O1 => "o1 Standard WETH",
            Self::Umbrae => "Umbrae",
            Self::Antfarm => "Antfarm",
            Self::Bundles=>"Bundles Swap",
            Self::Dagrow => "DAGrow",
            Self::Temple => "TempleDAO",
            Self::SafeSwap => "SafeSwap",
            Self::Pendle => "Pendle",
            Self::Carbon => "Carbon",
            Self::MintClub => "Mint Club",
        }
    }
    pub(super) async fn plan(
        self,
        ctx: &Ctx,
        s: &Step,
        h: &Hop,
        (wallet, height): (Address, u64),
        buy: bool,
        selected: Selection,
    ) -> GatewayResult<Plan> {
        match self {
            Self::SolidlyRouter => Box::pin(super::solidly_router_wallet::plan(ctx,s,wallet,height,selected.target)).await,
            Self::Ranges=>Box::pin(super::ranges_wallet::plan(ctx,s,wallet,height,selected.target)).await,
            Self::RegisteredV3 => Box::pin(super::registered_v3_wallet::plan(ctx,s,wallet,height,selected.target)).await,
            Self::O1 if s.entry.family.variant.flux_native() && selected.flux_eth=> Box::pin(super::flux_eth_plan::plan(ctx,s,wallet,height,selected.target)).await,
            Self::O1 => Box::pin(super::o1_plan::plan(ctx,s,wallet,height,selected.target)).await,
            Self::Umbrae => {
                Box::pin(super::umbrae::plan(
                    ctx,
                    s,
                    wallet,
                    height,
                    selected.target,
                    false,
                ))
                .await
            }
            Self::Antfarm if selected.proceeds.is_some() => {
                let (tier, loan) = selected.proceeds.unwrap();
                Box::pin(super::antfarm_output_wallet::plan(
                    ctx,
                    s,
                    wallet,
                    height,
                    selected.target,
                    tier,
                    loan,
                ))
                .await
            }
            Self::Antfarm => {
                Box::pin(super::antfarm_wallet::plan(
                    ctx,
                    s,
                    wallet,
                    height,
                    selected.target,
                ))
                .await
            }
            Self::Bundles=>Box::pin(super::bundles_plan::plan(ctx,s,wallet,height,selected.target)).await,
            Self::SafeSwap=>Box::pin(super::safeswap_wallet::plan(ctx,s,wallet,height,selected.target)).await,
            Self::Pendle=>Box::pin(super::pendle_wallet::plan(ctx,s,wallet,height,selected.target)).await,
            Self::Dagrow=>Box::pin(super::dagrow_wallet::plan(ctx,s,wallet,height,selected.dagrow)).await,
            Self::Temple => {
                Box::pin(super::temple_wallet::plan(
                    ctx,
                    s,
                    wallet,
                    height,
                    selected.target,
                ))
                .await
            }
            Self::Carbon => {
                Box::pin(super::carbon_direct::path_plan(
                    ctx,
                    s,
                    h,
                    wallet,
                    height,
                    buy,
                    (selected.target, selected.carbon),
                ))
                .await
            }
            Self::MintClub => {
                Box::pin(super::mint_club_direct::path_plan(
                    ctx,
                    s,
                    h,
                    wallet,
                    height,
                    selected.target,
                ))
                .await
            }
        }
    }
}

/// Router01/V3 also support V5 graph execution. Their native single-call form and the
/// dedicated Umbrae/SectorOne wallets require exactly one ERC20-to-ERC20 hop.
pub(super) fn router_shape(
    q:&super::super::quote::Quoting,route:&crate::core::types::DexRoute,leg:&crate::core::types::RouteLeg,
    native_asset:bool,cash:Address,
) -> GatewayResult<()> {
    let direct=route.mode==crate::core::types::RouteMode::Direct;
    if let [hop]=leg.hops.as_slice() && let Some(crate::core::types::RouterFeeMode::FluxEth{..})=hop.fee_mode {
        if direct && route.legs.len()==1 && (native_asset || super::native::cash(&q.ctx,&q.ctx.text("venue.quote_asset")?,cash)) && q.reg.entry(&hop.pool).is_some_and(|e|e.family.variant.flux_native()) {return Ok(());}
        return Err(q.ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"Omni Flux: поручение требует физический ETH и один рынок"));
    }
    let needs_wallet=leg.hops.iter().filter_map(|h|q.reg.entry(&h.pool)).any(|e|
        (direct && (matches!(e.family.variant,Variant::Dagrow|Variant::Bundles|Variant::PuzzleRanges|Variant::SafeSwapTrade) || e.family.variant.exact_router() || super::o1_wallet::enabled(&e))) || e.family.variant.umbrae() || super::super::pools::sectorone::enabled(&e));
    if needs_wallet && (!direct || leg.hops.len()!=1 || route.legs.len()!=1 || native_asset
        || super::native::cash(&q.ctx,&q.ctx.text("venue.quote_asset")?,cash)) {
        return Err(q.ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"Прямой роутер пула: требуется один шаг между ERC-20 кошелька"));
    }
    Ok(())
}

type ProbeCall = (Vec<Approval>, Address, Bytes);
pub(in crate::venues::bsc_dex) fn probe(
    ctx: &Ctx,
    e: &Entry,
    order: &Order,
    wallet: Address,
    now: u64,
    protection: Option<Protection>,
) -> Option<GatewayResult<ProbeCall>> {
    match (&e.pool.state, e.family.variant) {
        (PoolState::Antfarm(_), _) => Some(super::super::pools::antfarm_probe_wallet::part(
            ctx, e, order, now,
        )),
        (_,Variant::Bundles)=>Some(super::bundles_plan::probe(ctx,e,order,wallet,now)),
        (_,Variant::SafeSwapTrade)=>Some(super::safeswap_wallet::probe(ctx,e,order,wallet,now)),
        (_,Variant::Pendle) if e.family.executor_kind.is_some()=>Some(super::pendle_wallet::probe(ctx,e,order,now)),
        (_,Variant::Dagrow)=>Some(super::dagrow_wallet::probe(ctx,e,order,wallet,now)),
        (_, Variant::Temple) => Some(super::temple_wallet::probe(ctx, e, order, wallet, now)),
        (PoolState::MintClub(_), _) => Some(super::mint_club_direct::probe(
            ctx, e, order, wallet, protection,
        )),
        (_, Variant::EkuboV2) => Some(super::ekubo_v2_direct::probe(ctx, e, order, now)),
        _ => None,
    }
}
