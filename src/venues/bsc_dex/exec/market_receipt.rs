//! Разбор исполненного прямого обмена, в том числе после восстановления.
//! Семейство задаётся сохранённым поручением кошелька, а не текущими настройками.
use super::super::{ctx::Ctx, send::track::Receipt};
use super::{
    plan::{DirectCall, Plan},
    swap_ctx::SwapCtx,
};
use crate::gateway::errors::GatewayResult;
use alloy_primitives::{Address, B256, U256};

pub(super) fn capture(c: &mut SwapCtx, p: &Plan, wallet: Address) {
    c.vista = super::vista_direct::proof(p);
    c.ekubo = p.ekubo_proof();
    match p.direct_market() {
        Some(DirectCall::SolidlyRouter(a)) => c.solidly_router = Some(Box::new((*a).clone())),
        Some(DirectCall::Ranges(a))=>c.ranges=Some(Box::new((*a).clone())),
        Some(DirectCall::RegisteredV3(a)) => c.registered_v3 = Some(Box::new((*a).clone())),
        Some(DirectCall::AntfarmOutput(a)) => c.antfarm_output = Some(Box::new((*a).clone())),
        Some(DirectCall::Antfarm(a)) => c.antfarm = Some(Box::new((*a).clone())),
        Some(DirectCall::FluxEth(t))=>c.flux_eth=Some(Box::new((*t).clone())),
        Some(DirectCall::O1(t)) => c.o1 = Some(Box::new((*t).clone())),
        Some(DirectCall::Bundles(t))=>c.bundles=Some(Box::new((*t).clone())),
        Some(DirectCall::BancorLegacy(t))=>c.bancor_legacy=Some(Box::new((*t).clone())),
        Some(DirectCall::YieldBasis(t))=>c.yield_basis=Some(Box::new((*t).clone())),
        Some(DirectCall::BalancerV1(t))=>c.balancer_v1=Some(Box::new((*t).clone())),
        Some(DirectCall::BancorV3(t))=>c.bancor_v3=Some(Box::new((*t).clone())),
        Some(DirectCall::UniswapV1(t))=>c.uniswap_v1=Some(Box::new((*t).clone())),
        Some(DirectCall::Rubicon(t))=>c.rubicon=Some(Box::new((*t).clone())),
        Some(DirectCall::NativeFlat(t))=>c.native_flat=Some(Box::new((*t).clone())),
        Some(DirectCall::Prop(t))=>c.prop=Some(Box::new((*t).clone())),
        Some(DirectCall::Empire(t))=>c.empire=Some(Box::new((*t).clone())),
        Some(DirectCall::Angstrom(t))=>c.angstrom=Some(Box::new((*t).clone())),
        Some(DirectCall::Stabull(t))=>c.stabull=Some(Box::new((*t).clone())),
        Some(DirectCall::Integral(t))=>c.integral=Some(Box::new((*t).clone())),
        Some(DirectCall::Dagrow(t)) => c.dagrow=Some(Box::new((*t).clone())),
        Some(DirectCall::SafeSwap(t)) => c.safeswap=Some(Box::new((*t).clone())),
        Some(DirectCall::Pendle(t)) => c.pendle = Some(Box::new((*t).clone())),
        Some(DirectCall::Temple(t)) => c.temple = Some(Box::new((*t).clone())),
        Some(DirectCall::Carbon(a)) => {
            c.carbon = Some(super::carbon_fact::Proof {
                call: (*a).clone(),
                payer: wallet,
            })
        }
        Some(DirectCall::MintClub(call)) => {
            c.mint_club = Some(super::mint_club_fact::Proof {
                call,
                bond: call.protection.map_or(p.router, |v| v.bond),
                payer: wallet,
            })
        }
        _ => {}
    }
}
#[derive(Clone, Copy)]
pub(super) enum Saved<'a> {
    Gladius(&'a super::super::gladius::settlement::Saved),
    Raindex(&'a super::super::raindex::settlement::Saved),
    Epsilon(&'a super::super::epsilon::wire::Saved),
    SolidlyRouter(&'a super::solidly_router_wallet::Proof),
    Ranges(&'a super::ranges_wallet::Proof),
    RegisteredV3(&'a super::registered_v3_wallet::Proof),
    AntfarmOutput(&'a super::antfarm_output_wallet::Proof),
    Antfarm(&'a super::antfarm_wallet::Proof),
    O1(&'a super::o1_wallet::Proof),
    FluxEth(&'a super::flux_eth::Proof),
    Bundles(&'a super::bundles_wallet::Proof),
    BancorLegacy(&'a super::bancor_legacy_wallet::Proof),
    YieldBasis(&'a super::yield_basis_wallet::Proof),
    BalancerV1(&'a super::balancer_v1_wallet::Proof),
    BancorV3(&'a super::bancor_v3_wallet::Proof),
    UniswapV1(&'a super::uniswap_v1_wallet::Proof),
    Rubicon(&'a super::rubicon_wallet::Proof),
    NativeFlat(&'a super::mars_wallet::Proof),
    Prop(&'a super::prop_wallet::Proof),
    Empire(&'a super::empire_wallet::Proof),
    Angstrom(&'a super::angstrom_wallet::Proof),
    Stabull(&'a super::stabull_wallet::Proof),
    Integral(&'a super::integral_wallet::Proof),
    Dagrow(&'a super::dagrow_wallet::Proof),
    SafeSwap(&'a super::safeswap_wallet::Proof),
    Pendle(&'a super::pendle_wallet::Proof),
    Temple(&'a super::temple_wallet::Proof),
    Carbon(&'a super::carbon_fact::Proof),
    MintClub(&'a super::mint_club_fact::Proof),
    Ekubo(&'a super::ekubo_v2_fact::Proof),
    Vista(&'a super::vista_fact::Proof),
}
fn entries(c: &SwapCtx) -> impl Iterator<Item = Saved<'_>> {
    [
        c.gladius.as_deref().map(Saved::Gladius),
        c.raindex.as_deref().map(Saved::Raindex),
        c.epsilon.as_deref().map(Saved::Epsilon),
        c.solidly_router.as_deref().map(Saved::SolidlyRouter),
        c.ranges.as_deref().map(Saved::Ranges),
        c.registered_v3.as_deref().map(Saved::RegisteredV3),
        c.antfarm_output.as_deref().map(Saved::AntfarmOutput),
        c.antfarm.as_deref().map(Saved::Antfarm),
        c.o1.as_deref().map(Saved::O1),
        c.flux_eth.as_deref().map(Saved::FluxEth),
        c.bundles.as_deref().map(Saved::Bundles),
        c.bancor_legacy.as_deref().map(Saved::BancorLegacy),
        c.yield_basis.as_deref().map(Saved::YieldBasis),
        c.balancer_v1.as_deref().map(Saved::BalancerV1),
        c.bancor_v3.as_deref().map(Saved::BancorV3),
        c.uniswap_v1.as_deref().map(Saved::UniswapV1),
        c.rubicon.as_deref().map(Saved::Rubicon),
        c.native_flat.as_deref().map(Saved::NativeFlat),
        c.prop.as_deref().map(Saved::Prop),
        c.empire.as_deref().map(Saved::Empire),
        c.angstrom.as_deref().map(Saved::Angstrom),
        c.stabull.as_deref().map(Saved::Stabull),
        c.integral.as_deref().map(Saved::Integral),
        c.dagrow.as_deref().map(Saved::Dagrow),
        c.pendle.as_deref().map(Saved::Pendle),
        c.safeswap.as_deref().map(Saved::SafeSwap),
        c.temple.as_deref().map(Saved::Temple),
        c.carbon.as_ref().map(Saved::Carbon),
        c.mint_club.as_ref().map(Saved::MintClub),
        c.ekubo.as_ref().map(Saved::Ekubo),
        c.vista.as_ref().map(Saved::Vista),
    ]
    .into_iter()
    .flatten()
}
pub(super) fn first(c: &SwapCtx) -> Option<Saved<'_>> {
    entries(c).next()
}
pub(in crate::venues::bsc_dex) async fn included(
    c: &SwapCtx,
    ctx: &Ctx,
    hash: B256,
    nonce: Option<u64>,
    receipt: &Receipt,
) -> GatewayResult<()> {
    if let Some(p)=c.empire.as_deref() {
        let bad=||ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,"Empire: нет исходного хеша или nonce");
        let nonce=nonce.or_else(||c.chain.as_ref().map(|v|v.nonce)).ok_or_else(bad)?;
        if c.chain.as_ref().is_some_and(|v|(v.hash,v.nonce)!=(hash,nonce)) {return Err(bad());}
        return p.signed().included(ctx,hash,nonce,receipt).await;
    }
    if let Some(p)=c.angstrom.as_deref() {
        let bad=||ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,"Angstrom: исходная транзакция или блок выплаты не подтверждены");
        let nonce=nonce.or_else(||c.chain.as_ref().map(|prior|prior.nonce)).ok_or_else(bad)?;
        if c.chain.as_ref().is_some_and(|prior| (prior.hash,prior.nonce)!=(hash,nonce)) || (receipt.status.to::<u64>()==1 && receipt.block_number.to::<u64>()!=p.terms.height){return Err(bad());}
        return p.signed().included(ctx,hash,nonce,receipt).await;
    }
    if let Some(p) = c.registered_v3.as_deref().filter(|p| p.market.maverick) {
        let fail = || ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,"Maverick V2: нет подтверждения исходной сети, хеша или nonce");
        let (signed, nonce)=super::maverick_two_wire::origin(p,c.chain.as_ref().map(|v|(v.hash,v.nonce)),hash,nonce).ok_or_else(fail)?;
        return signed.included(ctx,hash,nonce,receipt).await;
    }
    if let Some(p)=&c.rfq{
        let chain=c.chain.as_ref().ok_or_else(||ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,"Нет исходного хеша или nonce Hashflow"))?;
        if chain.hash!=hash{return Err(ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,"Хеш Hashflow не совпал с исходным поручением"));}
        p.included(ctx,hash,chain.nonce,receipt).await?;
    }
    if let Some(g)=&c.gladius{
        let nonce=c.chain.as_ref().ok_or_else(||ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,"Нет исходного nonce Gladius"))?.nonce;
        g.included(ctx,hash,nonce,receipt).await?;
    }
    if let Some(g)=&c.epsilon{
        let nonce=c.chain.as_ref().ok_or_else(||ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,"Нет исходного nonce Epsilon"))?.nonce;
        g.included(ctx,hash,nonce,receipt).await?;
    }
    if let Some(g)=&c.raindex{
        let nonce=c.chain.as_ref().ok_or_else(||ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,"Нет исходного nonce Raindex"))?.nonce;
        g.included(ctx,hash,nonce,receipt).await?;
    }
    if let Some(n)=&c.native_rfq{
        let nonce=c.chain.as_ref().ok_or_else(||ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,"Нет исходного nonce Native RFQ"))?.nonce;
        n.included(ctx,hash,nonce,receipt).await?;
    }
    if receipt.status.to::<u64>() != 1 {
        return Ok(());
    }
    for saved in entries(c) {
        match saved {
            Saved::Gladius(_) => {}, // в том числе проверено для revert выше
            Saved::Raindex(_) | Saved::Epsilon(_) => {}, // в том числе проверено для revert выше
            Saved::SolidlyRouter(p) => p.included(ctx, hash, receipt).await?,
            Saved::Ranges(p)=>p.included(ctx,hash,receipt).await?,
            Saved::RegisteredV3(p) => p.included(ctx, hash, receipt).await?,
            Saved::AntfarmOutput(p) => p.included(ctx, hash, receipt).await?,
            Saved::Antfarm(p) => p.included(ctx, hash, receipt).await?,
            Saved::FluxEth(p)=>p.included(ctx,hash,receipt).await?,
            Saved::O1(p) => p.included(ctx, hash, receipt).await?,
            Saved::Bundles(p)=>p.included(ctx,hash,receipt).await?,
            Saved::BancorLegacy(p)=>p.included(ctx,hash,receipt).await?,
            Saved::YieldBasis(p)=>p.included(ctx,hash,receipt).await?,
            Saved::BalancerV1(p)=>p.included(ctx,hash,receipt).await?,
            Saved::BancorV3(p)=>p.included(ctx,hash,receipt).await?,
            Saved::UniswapV1(p)=>p.included(ctx,hash,receipt).await?,
            Saved::Rubicon(p)=>p.included(ctx,hash,receipt).await?,
            Saved::NativeFlat(p)=>p.included(ctx,hash,receipt).await?,
            Saved::Prop(p)=>p.included(ctx,hash,receipt).await?,
            Saved::Empire(_)=>return Err(ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,"Empire: нет исходного подтверждённого поручения")),
            Saved::Angstrom(_)=>return Err(ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,"Angstrom: нет исходного хеша для сверки подписанта")),
            Saved::Stabull(p)=>p.included(ctx,hash,receipt).await?,
            Saved::Integral(p)=>p.included(ctx,hash,receipt).await?,
            Saved::Dagrow(p)=>p.included(ctx,hash,receipt).await?,
            Saved::SafeSwap(p)=>p.included(ctx,hash,receipt).await?,
            Saved::Pendle(p) => p.included(ctx, hash, receipt).await?,
            Saved::Temple(p) => p.included(ctx, hash, receipt).await?,
            Saved::Carbon(p) => p.included(ctx, hash, receipt).await?,
            Saved::MintClub(p) => p.included(ctx, hash, receipt).await?,
            Saved::Ekubo(p) => p.included(ctx, hash, receipt).await?,
            Saved::Vista(p) => p.included(ctx, hash, receipt).await?,
        }
    }
    Ok(())
}
impl Saved<'_> {
    pub(super) fn amounts(
        self,
        r: &Receipt,
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Result<(U256, U256), &'static str> {
        match self {
            Self::Gladius(p) => p.amounts(&r.logs).ok_or("Gladius: фактические движения и выплата не подтверждены"),
            Self::Epsilon(p) => p.amounts(&r.logs).ok_or("Epsilon: не подтверждены чистая выдача, провайдер или расход кошелька"),
            Self::Raindex(p) => p.amounts(&r.logs).ok_or("Raindex: фактические движения и выплата не подтверждены"),
            Self::SolidlyRouter(p) => p.amounts(&r.logs,wallet,input,output).ok_or("Solidly Router01: не подтверждены расходы, получатель или выплата по исходному поручению"),
            Self::Ranges(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("Ranges: событие Vault и чистые движения кошелька расходятся с подписанным поручением"),
            Self::RegisteredV3(p) => p.amounts(&r.logs,wallet,input,output).ok_or("V3 Router: не подтверждены расходы, получатель или выплата по исходному поручению"),
            Self::AntfarmOutput(p) => p
                .amounts(&r.logs, wallet, input, output)
                .ok_or("Antfarm: комиссия из выплаты, выплата или списание по исходному поручению не подтверждены"),
            Self::Antfarm(p) => p
                .amounts(&r.logs, wallet, input, output)
                .ok_or("Antfarm: выплата или списание по исходному поручению не подтверждены"),
            Self::FluxEth(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("Omni Flux: физический ETH, PoolId или перевод токена не подтверждены"),
            Self::O1(p) => p.amounts(&r.logs, wallet, input, output).ok_or("o1: ключ, списание или выплата расходятся с подписанным поручением"),
            Self::Bundles(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("Bundles Swap: подписанное поручение и движения денег расходятся"),
            Self::BancorLegacy(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("BancorLegacy: поручение, событие пула или поступление кошельку не совпали"),
            Self::YieldBasis(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("YieldBasis: поручение, событие пула или поступление кошельку не совпали"),
            Self::BalancerV1(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("Balancer V1: расход, обмен BPool или выплата не совпали"),
            Self::BancorV3(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("Bancor V3: фондирование хранилища и выплата кошельку не совпали"),
            Self::UniswapV1(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("Uniswap V1: события обменов и выплата кошельку не совпали"),
            Self::Rubicon(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("Rubicon Classic: заявки или выплата кошельку не совпали"),
            Self::NativeFlat(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("MarSwap: подписанная пара или движение средств не совпали"),
            Self::Prop(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("LambdaClass: фактический источник и движение средств не подтвердили условия"),
            Self::Empire(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("Empire: исходный рынок, участники или выплата расходятся с измерением модуля"),
            Self::Angstrom(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("Angstrom: фактическая оплата или PoolId расходятся с подписанным поручением"),
            Self::Stabull(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("Stabull: поручение, Curve Trade и дельты кошелька не совпадают"),
            Self::Integral(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("Integral SIZE: поручение, событие или дельты кошелька не совпадают"),
            Self::Dagrow(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("DAGrow: исходные границы, получатель или движения кошелька не подтверждены"),
            Self::SafeSwap(p)=>p.amounts(&r.logs,wallet,input,output).ok_or("SafeSwap: фактические выплаты пула и кошелька расходятся с исходным поручением"),
            Self::Pendle(p) => p.amounts(&r.logs,wallet,input,output).ok_or("Pendle: рынок, плательщик или переданные активы расходятся с поручением"),
            Self::Temple(p) => p.amounts(&r.logs, wallet, input, output).ok_or(
                "TempleDAO: квитанция не подтвердила чистое списание и выплату исходной валюты",
            ),
            Self::Carbon(p) => p
                .amounts(&r.logs, wallet, input, output)
                .ok_or("Carbon: не подтверждены исходные стратегии и чистая выплата"),
            Self::MintClub(p) => p
                .amounts(&r.logs, wallet, input, output)
                .ok_or("Mint Club: расчёт по исходному поручению Bond не подтверждён"),
            Self::Ekubo(p) => p
                .amounts(&r.logs, wallet, input, output)
                .ok_or("Ekubo V2: лог Core не подтвердил выплату кошельку"),
            Self::Vista(p) => p
                .amounts(&r.logs, wallet, input, output)
                .ok_or("Квитанция EtherVista не подтверждает чистую выплату"),
        }
    }
}
