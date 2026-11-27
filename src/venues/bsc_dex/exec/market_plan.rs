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
    Ammalgam,
    Ranges,
    RegisteredV3,
    SolidlyRouter,
    Umbrae,
    O1,
    Antfarm,
    Dagrow,
    SafeSwap,
    Aktionariat,
    BancorLegacy,
    FluidDexLite,
    Ambient,
    Llamma,
    Motoswap,
    Mooniswap,
    DefiPlaza,
    YieldBasis,
    BalancerV1,
    BancorV3,
    UniswapV1,
    Rubicon,
    NativeFlat,
    Prop,
    Aqua,
    BaseStonk,
    Angstrom,
    EulerSwap,
    Stabull,
    Integral,
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
    bounds:Option<(Address,Option<alloy_primitives::U256>)>,
    proceeds: Option<(u16, bool)>,
    bancor_legacy:Option<(Address,bool)>,
    mooniswap:Option<(Address,bool)>,
    defiplaza:Option<(Address,bool)>,
    yield_basis:Option<(Address,bool)>,
    balancer_v1:Option<super::balancer_v1_plan::Selection>,
    bancor_v3:Option<(Address,super::bancor_v3_plan::Form)>,
    uniswap_v1:Option<(Address,bool,bool)>,
    flux_eth:bool,
    pub(super) fluid_hop: Option<(Address,alloy_primitives::U256)>,
    pub(super) native_flat:Option<(Address,alloy_primitives::U256)>,
    venue_fallback:Option<(Address,bool)>,
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
                bounds:None,
                proceeds: Some((tier, loan)),
                bancor_legacy:None,
                mooniswap:None,
                defiplaza:None,
                yield_basis:None,
                balancer_v1:None,
                bancor_v3:None,
                uniswap_v1:None,
                flux_eth:false,
                fluid_hop:None,
                native_flat:None,
                venue_fallback:None,
            };
        }
        Self {
            target: super::mint_club_direct::selected(leg),
            carbon: super::carbon_direct::selected(leg).1,
            bounds:match leg.hops.as_slice(){[h]=>match h.fee_mode{Some(crate::core::types::RouterFeeMode::Llamma{target,desired,v5:None,..}|crate::core::types::RouterFeeMode::WalletBounds{target,desired}|crate::core::types::RouterFeeMode::Dagrow{target,desired})=>Some((target,desired)),_=>None},_=>None},
            proceeds: None,
            fluid_hop:super::super::quote::fluid_hop_pricing::selected(&leg.hops),
            native_flat:match leg.hops.as_slice(){[h]=>match h.fee_mode{Some(crate::core::types::RouterFeeMode::NativeFlat{target,wei})=>Some((target,wei)),_=>None},_=>None},
            venue_fallback:match leg.hops.as_slice(){[h]=>match h.fee_mode{Some(crate::core::types::RouterFeeMode::VenueFallback{target,allowed})=>Some((target,allowed)),_=>None},_=>None},
            bancor_legacy:match leg.hops.as_slice(){[h]=>match h.fee_mode{Some(crate::core::types::RouterFeeMode::BancorLegacy{target,native,..})=>Some((target,native)),_=>None},_=>None},
            mooniswap:match leg.hops.as_slice(){[h]=>match h.fee_mode{Some(crate::core::types::RouterFeeMode::Mooniswap{target,native,..})=>Some((target,native)),_=>None},_=>None},
            defiplaza:match leg.hops.as_slice(){[h]=>match h.fee_mode{Some(crate::core::types::RouterFeeMode::DefiPlaza{target,native,..})=>Some((target,native)),_=>None},_=>None},
            yield_basis:match leg.hops.as_slice(){[h]=>match h.fee_mode{Some(crate::core::types::RouterFeeMode::YieldBasis{target,native,..})=>Some((target,native)),_=>None},_=>None},
            balancer_v1:match leg.hops.as_slice(){[h]=>match h.fee_mode{
                Some(crate::core::types::RouterFeeMode::BalancerV1{target,native,desired,..})=>
                    Some(super::balancer_v1_plan::Selection{target,native,desired}),_=>None},_=>None},
            bancor_v3:match leg.hops.as_slice(){[h]=>match h.fee_mode{Some(crate::core::types::RouterFeeMode::BancorV3{target,module,native,exempt,desired,..})=>Some((target,super::bancor_v3_plan::Form{module,native,exempt,desired})),_=>None},_=>None},
            uniswap_v1:match leg.hops.as_slice(){[h]=>match h.fee_mode{Some(crate::core::types::RouterFeeMode::UniswapV1{target,module,native,..})=>Some((target,module,native)),_=>None},_=>None},
            flux_eth:matches!(leg.hops.as_slice(),[hop] if matches!(hop.fee_mode,Some(crate::core::types::RouterFeeMode::FluxEth{..}))),
        }
    }
}
impl Market {
    pub(super) fn of(e: &Entry) -> Option<Self> {
        match &e.pool.state {
            _ if e.family.variant.classic_wallet() || matches!(e.family.variant,Variant::EmpireNominal|Variant::V2ExactRouter) => Some(Self::SolidlyRouter),
            _ if e.family.variant.ammalgam()=>Some(Self::Ammalgam),
            _ if e.family.variant==Variant::PuzzleRanges=>Some(Self::Ranges),
            _ if matches!(e.family.variant, Variant::RegisteredUniV3 | Variant::RegisteredSlipstream | Variant::RegisteredSpacingV3 | Variant::VvsV3 | Variant::ShibaswapV2 | Variant::CypherIntegral | Variant::TheoremIntegral | Variant::SlipstreamUniversal | Variant::MaverickTwoRouter) => Some(Self::RegisteredV3),
            _ if super::o1_wallet::enabled(e) => Some(Self::O1),
            _ if e.family.variant.umbrae() => Some(Self::Umbrae),
            PoolState::Antfarm(_) => Some(Self::Antfarm),
            PoolState::Bundles(_) => Some(Self::Bundles),
            _ if e.family.variant.protected_router_wallet() => Some(Self::SafeSwap),
            _ if e.family.variant==Variant::Pendle && e.family.executor_kind.is_some()=>Some(Self::Pendle),
            _ if e.family.variant==Variant::PropVenue=>Some(Self::Prop),
            _ if e.family.variant.aqua()=>Some(Self::Aqua),
            _ if e.family.variant==Variant::BaseStonk=>Some(Self::BaseStonk),
            _ if e.family.variant==Variant::Angstrom=>Some(Self::Angstrom),
            _ if e.family.variant.euler()=>Some(Self::EulerSwap),
            _ if e.family.variant==Variant::Stabull=>Some(Self::Stabull),
            _ if e.family.variant==Variant::IntegralSize=>Some(Self::Integral),
            PoolState::Dagrow(_) => Some(Self::Dagrow),
            PoolState::Temple(_) if e.family.executor_kind.is_some() => Some(Self::Temple),
            _ if e.family.variant.issuer_shares()=>Some(Self::Aktionariat),
            _ if e.family.variant==Variant::BancorLegacy=>Some(Self::BancorLegacy),
            _ if e.family.variant==Variant::FluidDexLite=>Some(Self::FluidDexLite),
            _ if e.family.variant.ambient()=>Some(Self::Ambient),
            _ if e.family.variant.llamma()=>Some(Self::Llamma),
            _ if e.family.variant==Variant::Motoswap=>Some(Self::Motoswap),
            _ if e.family.variant.mooniswap()=>Some(Self::Mooniswap),
            _ if e.family.variant.defiplaza()=>Some(Self::DefiPlaza),
            _ if e.family.variant==Variant::YieldBasis=>Some(Self::YieldBasis),
            PoolState::BalancerV1(_) => Some(Self::BalancerV1),
            PoolState::BancorV3(_) => Some(Self::BancorV3),
            PoolState::UniswapV1(_) => Some(Self::UniswapV1),
            PoolState::Rubicon(_) => Some(Self::Rubicon),
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
            (_,Self::BancorLegacy)=>e.family.variant==Variant::BancorLegacy,
            (_,Self::Mooniswap)=>e.family.variant.mooniswap(),
            (_,Self::DefiPlaza)=>e.family.variant.defiplaza(),
            (_,Self::YieldBasis)=>e.family.variant==Variant::YieldBasis,
            (PoolState::BalancerV1(_),Self::BalancerV1)=>true,
            (PoolState::BancorV3(_),Self::BancorV3)=>true,
            (PoolState::UniswapV1(s),Self::UniswapV1)=>!native || !s.dual(),
            (_,Self::O1) if e.family.variant.flux_native()=>true,
            (PoolState::Carbon(s), Self::Carbon) => s.native_side.is_some() == native,
            _ => !native,
        }
    }
    pub(super) fn missing_selection(self) -> &'static str {
        match self {
            Self::SolidlyRouter => "Solidly Router01: требуется исходный адрес исполнения",
            Self::Ammalgam=>"Ammalgam: требуется выбранный модуль кошелька",
            Self::Ranges=>"Ranges: требуется исходный адрес исполнения",
            Self::RegisteredV3 => "V3 Router: требуется исходный адрес исполнения",
            Self::O1 => "o1: выберите проверенный модуль для расчёта WETH",
            Self::Umbrae => "Umbrae: требуется выбранный адрес исполнения",
            Self::Antfarm => "Antfarm: требуется подтверждённый бюджет комиссии",
            Self::Bundles=>"Bundles Swap: требуется выбранный адрес исполнения",
            Self::Dagrow => "DAGrow: требуются исходные условия поручения",
            Self::SafeSwap => "SafeSwap: требуется выбранный модуль с оплатой из WETH",
            Self::Aktionariat=>"Aktionariat: требуется подтверждённое поручение на целые акции",
            Self::BancorLegacy=>"BancorLegacy: отсутствует исходное поручение кошелька",
            Self::FluidDexLite=>"Fluid DEX Lite: нет сохранённого поручения",
            Self::Ambient=>"Ambient: нет сохранённого поручения",
            Self::Llamma=>"LLAMMA: нет сохранённого поручения",
            Self::Motoswap=>"Motoswap: отсутствует выбранный плательщиком адрес исполнения",
            Self::Mooniswap=>"Mooniswap: отсутствует исходное поручение кошелька",
            Self::DefiPlaza=>"DefiPlaza: отсутствует исходное поручение кошелька",
            Self::YieldBasis=>"YieldBasis: отсутствует исходное поручение кошелька",
            Self::BalancerV1=>"Balancer V1: отсутствует поручение для выбранного BPool",
            Self::BancorV3=>"Bancor V3: отсутствует поручение для выбранного рынка",
            Self::UniswapV1=>"Uniswap V1: потеряна форма выбранного обмена",
            Self::Rubicon=>"Rubicon Classic: требуется подтверждённый модуль и заявки",
            Self::NativeFlat=>"MarSwap: требуется выбранная плата за одну пару",
            Self::Prop => "LambdaClass: требуется исходное поручение с выбранным адресом и разрешением перехода на Uniswap",
            Self::Aqua => "Aqua: точная программа, роль плательщика и оплата не подтверждены",
            Self::BaseStonk => "BaseStonk: требуется выбранный прямой модуль",
            Self::Angstrom => "Angstrom: требуется модуль и аттестация целевого блока",
            Self::EulerSwap => "EulerSwap: требуется выбранный пул и бюджет",
            Self::Stabull => "Stabull: требуется выбранная Curve и бюджет",
            Self::Integral => "Integral SIZE: требуется выбранная форма мгновенного поручения",
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
            Self::Ammalgam=>"Ammalgam",
            Self::Ranges=>"Ranges",
            Self::RegisteredV3 => "V3 Router",
            Self::O1 => "o1 Standard WETH",
            Self::Umbrae => "Umbrae",
            Self::Antfarm => "Antfarm",
            Self::Bundles=>"Bundles Swap",
            Self::Dagrow => "DAGrow",
            Self::Temple => "TempleDAO",
            Self::SafeSwap => "SafeSwap",
            Self::Aktionariat=>"Aktionariat",
            Self::BancorLegacy=>"BancorLegacy",
            Self::FluidDexLite=>"Fluid DEX Lite",
            Self::Ambient=>"Ambient",
            Self::Llamma=>"LLAMMA",
            Self::Motoswap=>"Motoswap",
            Self::Mooniswap=>"Mooniswap",
            Self::DefiPlaza=>"DefiPlaza",
            Self::YieldBasis=>"YieldBasis",
            Self::BalancerV1=>"Balancer V1",
            Self::BancorV3=>"Bancor V3",
            Self::UniswapV1=>"Uniswap V1",
            Self::Rubicon=>"Rubicon Classic",
            Self::NativeFlat=>"MarSwap: фиксированная плата ETH",
            Self::Prop => "LambdaClass PropAMMRouter",
            Self::Aqua => "Aqua",
            Self::BaseStonk => "BaseStonk",
            Self::Angstrom => "Angstrom",
            Self::EulerSwap => "EulerSwap",
            Self::Stabull => "Stabull",
            Self::Integral => "Integral SIZE",
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
            Self::Ammalgam=>Box::pin(super::solidly_direct::ammalgam(ctx,s,wallet,height,selected.target)).await,
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
            Self::Aktionariat=>Box::pin(super::aktionariat_plan::plan(ctx,s,wallet,height,selected.target)).await,
            Self::BancorLegacy=>Box::pin(super::bancor_legacy_plan::plan(ctx,s,wallet,height,selected.bancor_legacy)).await,
            Self::FluidDexLite=>Box::pin(super::fluid_lite_plan::plan(ctx,s,wallet,height,selected.bounds)).await,
            Self::Ambient=>Box::pin(super::ambient_plan::plan(ctx,s,wallet,height,selected.bounds)).await,
            Self::Llamma=>Box::pin(super::llamma_plan::plan(ctx,s,wallet,height,selected.bounds)).await,
            Self::Motoswap=>Box::pin(super::motoswap_plan::plan(ctx,s,wallet,height,selected.target)).await,
            Self::Mooniswap=>Box::pin(super::mooniswap_plan::plan(ctx,s,wallet,height,selected.mooniswap)).await,
            Self::DefiPlaza=>Box::pin(super::defiplaza_plan::plan(ctx,s,wallet,height,selected.defiplaza)).await,
            Self::YieldBasis=>Box::pin(super::yield_basis_plan::plan(ctx,s,wallet,height,selected.yield_basis)).await,
            Self::BalancerV1=>Box::pin(super::balancer_v1_plan::plan(ctx,s,wallet,height,selected.balancer_v1)).await,
            Self::BancorV3=>Box::pin(super::bancor_v3_plan::plan(ctx,s,wallet,height,selected.bancor_v3)).await,
            Self::UniswapV1=>Box::pin(super::uniswap_v1_plan::plan(ctx,s,wallet,height,selected.uniswap_v1)).await,
            Self::Rubicon=>Box::pin(super::rubicon_plan::plan(ctx,s,wallet,height,selected.bounds)).await,
            Self::NativeFlat=>Box::pin(super::mars_plan::plan(ctx,s,wallet,height,selected.native_flat.ok_or_else(||ctx.err(crate::gateway::errors::ErrorCode::NoRoute,self.missing_selection()))?)).await,
            Self::Prop=>Box::pin(super::prop_plan::plan(ctx,s,wallet,height,selected.venue_fallback)).await,
            Self::Aqua=>Box::pin(super::aqua_plan::plan(ctx,s,wallet,height,selected.target)).await,
            Self::BaseStonk=>Box::pin(super::basestonk_plan::plan(ctx,s,wallet,height,selected.target)).await,
            Self::Angstrom=>Box::pin(super::angstrom_plan::plan(ctx,s,wallet,height,selected.target)).await,
            Self::EulerSwap | Self::Stabull=>Box::pin(super::stabull_plan::plan(ctx,s,wallet,height,selected.bounds)).await,
            Self::Integral=>Box::pin(super::integral_plan::plan(ctx,s,wallet,height,selected.bounds)).await,
            Self::Dagrow=>Box::pin(super::dagrow_wallet::plan(ctx,s,wallet,height,selected.bounds)).await,
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
    if leg.hops.iter().any(|h|matches!(h.fee_mode,Some(crate::core::types::RouterFeeMode::FluidHop{..}|crate::core::types::RouterFeeMode::MotoswapPath{..}))) {
        if let Some((target,_))=super::super::quote::fluid_hop_pricing::selected(&leg.hops)
          && direct && route.legs.len()==1 && !native_asset && !super::native::cash(&q.ctx,&q.ctx.text("venue.quote_asset")?,cash)
          && leg.hops.iter().all(|h|q.reg.entry(&h.pool).is_some_and(|e|e.family.id==h.family && e.pool.kind==h.kind && match h.fee_mode{
            Some(crate::core::types::RouterFeeMode::MotoswapPath{..})=>super::motoswap_plan::shape(&e).is_some() && [q.ctx.dex_addr("dex.router",e.family.id).ok(),q.ctx.dex_addr("dex.quoter",e.family.id).ok()].contains(&Some(target)),
            _=>super::super::pools::fluid_dex_lite::shape(&e).is_some() && q.ctx.dex_addr("dex.router",e.family.id).ok()==Some(target)
           })){return Ok(());}
        return Err(super::super::pools::fluid_dex_lite::bad(&q.ctx));
    }
    if let [hop]=leg.hops.as_slice() && let Some(crate::core::types::RouterFeeMode::EulerModuleV5{module,executor})=hop.fee_mode {
        if !direct && !native_asset && route.legs.len()==1 && route.mode==crate::core::types::RouteMode::Graph
            && q.reg.entry(&hop.pool).is_some_and(|e|e.family.variant.euler() && q.ctx.dex_addr("dex.router",e.family.id).ok()==Some(module))
            && super::executor::takes(&q.ctx,&q.reg,route,false)?==Some(executor){return Ok(());}
        return Err(q.ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"EulerSwap: изменились исполнитель, валюта или модуль"));
    }
    if let [hop]=leg.hops.as_slice() && let Some(crate::core::types::RouterFeeMode::FluxEth{..})=hop.fee_mode {
        if direct && route.legs.len()==1 && (native_asset || super::native::cash(&q.ctx,&q.ctx.text("venue.quote_asset")?,cash)) && q.reg.entry(&hop.pool).is_some_and(|e|e.family.variant.flux_native()) {return Ok(());}
        return Err(q.ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"Omni Flux: поручение требует физический ETH и один рынок"));
    }
    if let [hop]=leg.hops.as_slice() && let Some(crate::core::types::RouterFeeMode::BancorLegacy{native,v5,..})=hop.fee_mode {
        if let Some(expected)=v5 {
            if !direct && !native && route.mode==crate::core::types::RouteMode::Graph && route.legs.len()==1
                && super::executor::takes(&q.ctx,&q.reg,route,native_asset)?==Some(expected) {return Ok(());}
            return Err(q.ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"BancorLegacy: изменился выбранный исполнитель V5"));
        }
        if direct && route.legs.len()==1 && native==(native_asset || super::native::cash(&q.ctx,&q.ctx.text("venue.quote_asset")?,cash))
            && q.reg.entry(&hop.pool).is_some_and(|e|e.family.variant==Variant::BancorLegacy){return Ok(());}
        return Err(q.ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"BancorLegacy: изменилась валюта кошелька"));
    }
    if let [hop]=leg.hops.as_slice() && let Some(crate::core::types::RouterFeeMode::Llamma{target,v5,maximum,desired})=hop.fee_mode {
        let entry=q.reg.entry(&hop.pool).filter(|e|super::super::pools::llamma::shape(e).is_some());
        let erc20=!native_asset&&!super::native::cash(&q.ctx,&q.ctx.text("venue.quote_asset")?,cash);
        if erc20 && route.legs.len()==1 && !maximum.is_zero() && let Some(e)=entry {
            if let Some(exec)=v5 {
                if !direct && desired.is_none() && target==e.pool.reference.address() && route.mode==crate::core::types::RouteMode::Graph
                    && super::executor::takes(&q.ctx,&q.reg,route,false)?==Some(exec){return Ok(())}
            }else if direct && e.family.variant.llamma() && super::llamma_plan::terms(&q.ctx,&e,target,desired).is_ok(){return Ok(())}
        }
        return Err(q.ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"LLAMMA: изменились валюта, пул или исполнитель исходного поручения"));
    }
    if let [hop]=leg.hops.as_slice() && let Some(crate::core::types::RouterFeeMode::YieldBasis{native,v5,..})=hop.fee_mode {
        if let Some(expected)=v5 {
            if !direct && !native && route.mode==crate::core::types::RouteMode::Graph && route.legs.len()==1
                && super::executor::takes(&q.ctx,&q.reg,route,native_asset)?==Some(expected) {return Ok(());}
            return Err(q.ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"YieldBasis: изменился выбранный исполнитель V5"));
        }
        if direct && route.legs.len()==1 && native==(native_asset || super::native::cash(&q.ctx,&q.ctx.text("venue.quote_asset")?,cash))
            && q.reg.entry(&hop.pool).is_some_and(|e|e.family.variant==Variant::YieldBasis){return Ok(());}
        return Err(q.ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"YieldBasis: изменилась валюта кошелька"));
    }
    if let [hop]=leg.hops.as_slice() && let Some(crate::core::types::RouterFeeMode::Mooniswap{native,..})=hop.fee_mode {
        if direct && route.legs.len()==1 && native==(native_asset || super::native::cash(&q.ctx,&q.ctx.text("venue.quote_asset")?,cash))
            && q.reg.entry(&hop.pool).is_some_and(|e|matches!(e.pool.state,PoolState::Mooniswap(_))){return Ok(());}
        return Err(q.ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"Mooniswap: изменилась валюта кошелька"));
    }
    if let [hop]=leg.hops.as_slice() && let Some(crate::core::types::RouterFeeMode::DefiPlaza{native,..})=hop.fee_mode {
        if direct && route.legs.len()==1 && native==(native_asset || super::native::cash(&q.ctx,&q.ctx.text("venue.quote_asset")?,cash))
            && q.reg.entry(&hop.pool).is_some_and(|e|matches!(e.pool.state,PoolState::DefiPlaza(_))){return Ok(());}
        return Err(q.ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"DefiPlaza: изменилась валюта кошелька"));
    }
    if let [hop]=leg.hops.as_slice() && let Some(crate::core::types::RouterFeeMode::BalancerV1{native,..})=hop.fee_mode {
        if direct && route.legs.len()==1 && native==(native_asset || super::native::cash(&q.ctx,&q.ctx.text("venue.quote_asset")?,cash))
            && q.reg.entry(&hop.pool).is_some_and(|e|matches!(e.pool.state,PoolState::BalancerV1(_))){return Ok(());}
        return Err(q.ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"Balancer V1: изменилась валюта кошелька"));
    }
    if let [hop]=leg.hops.as_slice() && let Some(crate::core::types::RouterFeeMode::BancorV3{native,..})=hop.fee_mode {
        if direct && route.legs.len()==1 && native==(native_asset || super::native::cash(&q.ctx,&q.ctx.text("venue.quote_asset")?,cash)) && q.reg.entry(&hop.pool).is_some_and(|e|matches!(e.pool.state,PoolState::BancorV3(_))){return Ok(());}
        return Err(q.ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"Bancor V3: валюта подписываемого поручения изменилась"));
    }
    if let [hop]=leg.hops.as_slice() && let Some(crate::core::types::RouterFeeMode::UniswapV1{native,..})=hop.fee_mode {
        if direct && route.legs.len()==1 && native==(native_asset || super::native::cash(&q.ctx,&q.ctx.text("venue.quote_asset")?,cash)) && q.reg.entry(&hop.pool).is_some_and(|e|matches!(e.pool.state,PoolState::UniswapV1(_))){return Ok(());}
        return Err(q.ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"Uniswap V1: валюта кошелька или адрес рынка изменились"));
    }
    let native_flat=leg.hops.iter().any(|h|matches!(h.fee_mode,Some(crate::core::types::RouterFeeMode::NativeFlat{..})));
    let needs_wallet=native_flat||leg.hops.iter().filter_map(|h|q.reg.entry(&h.pool)).any(|e|
        (direct && (e.family.variant==Variant::Motoswap || e.family.variant.euler() || e.family.variant.llamma() || e.family.variant.aqua() || e.family.variant.ambient() || matches!(e.family.variant,Variant::FluidDexLite|Variant::Brokerbot|Variant::BaseStonk|Variant::Angstrom|Variant::RubiconClassic|Variant::PropVenue|Variant::IntegralSize|Variant::Stabull|Variant::Dagrow|Variant::Bundles|Variant::PuzzleRanges|Variant::SafeSwapTrade|Variant::DegenSwap) || e.family.variant.exact_router() || super::o1_wallet::enabled(&e))) || e.family.variant.umbrae() || super::super::pools::sectorone::enabled(&e));
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
        (_,variant) if variant.protected_router_wallet()=>Some(super::safeswap_wallet::probe(ctx,e,order,wallet,now)),
        (_,Variant::Pendle) if e.family.executor_kind.is_some()=>Some(super::pendle_wallet::probe(ctx,e,order,now)),
        (_,Variant::Brokerbot)=>Some(super::aktionariat_plan::probe(ctx,e,order,wallet,now)),
        (_,Variant::BancorLegacy)=>Some(super::bancor_legacy_plan::probe(ctx,e,order,wallet,now)),
        (_,Variant::Motoswap)=>Some(super::motoswap_plan::probe(ctx,e,order,wallet,now)),
        (_,Variant::Mooniswap)=>Some(super::mooniswap_plan::probe(ctx,e,order,wallet,now)),
        (_,v) if v.defiplaza()=>Some(super::defiplaza_plan::probe(ctx,e,order,wallet,now)),
        (_,Variant::YieldBasis)=>Some(super::yield_basis_plan::probe(ctx,e,order,wallet,now)),
        (PoolState::BalancerV1(_),_)=>Some(super::balancer_v1_plan::probe(ctx,e,order,wallet,now)),
        (PoolState::BancorV3(s),_)=>Some(super::bancor_v3_plan::probe(ctx,e,s,order,wallet,now)),
        (PoolState::UniswapV1(s),_)=>Some(super::uniswap_v1_plan::probe(ctx,e,s,order,wallet,now)),
        (PoolState::Rubicon(book),_)=>Some(super::rubicon_plan::probe(ctx,e,book,order,wallet,now)),
        (_,Variant::PropVenue)=>Some(super::prop_plan::probe(ctx,e,order,wallet,now)),
        (_,v) if v.aqua()=>Some(super::aqua_plan::probe(ctx,e,order,wallet,now)),
        (_,Variant::BaseStonk)=>Some(super::basestonk_plan::probe(ctx,e,order,wallet,now)),
        (_,Variant::Angstrom)=>Some(super::angstrom_plan::probe(ctx,e,order,wallet,now)),
        (_,v) if v.euler()=>Some(super::stabull_plan::probe(ctx,e,order,wallet,now)),
        (_,Variant::Stabull)=>Some(super::stabull_plan::probe(ctx,e,order,wallet,now)),
        (_,Variant::IntegralSize)=>Some(super::integral_plan::probe(ctx,e,order,wallet,now)),
        (_,Variant::Dagrow)=>Some(super::dagrow_wallet::probe(ctx,e,order,wallet,now)),
        (_, Variant::Temple) => Some(super::temple_wallet::probe(ctx, e, order, wallet, now)),
        (PoolState::MintClub(_), _) => Some(super::mint_club_direct::probe(
            ctx, e, order, wallet, protection,
        )),
        (_, Variant::EkuboV2) => Some(super::ekubo_v2_direct::probe(ctx, e, order, now)),
        _ => None,
    }
}
