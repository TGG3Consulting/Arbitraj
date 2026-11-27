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
    SolidlyRouter(&'a super::solidly_router_wallet::Proof),
    Ranges(&'a super::ranges_wallet::Proof),
    RegisteredV3(&'a super::registered_v3_wallet::Proof),
    AntfarmOutput(&'a super::antfarm_output_wallet::Proof),
    Antfarm(&'a super::antfarm_wallet::Proof),
    O1(&'a super::o1_wallet::Proof),
    FluxEth(&'a super::flux_eth::Proof),
    Bundles(&'a super::bundles_wallet::Proof),
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
        c.solidly_router.as_deref().map(Saved::SolidlyRouter),
        c.ranges.as_deref().map(Saved::Ranges),
        c.registered_v3.as_deref().map(Saved::RegisteredV3),
        c.antfarm_output.as_deref().map(Saved::AntfarmOutput),
        c.antfarm.as_deref().map(Saved::Antfarm),
        c.o1.as_deref().map(Saved::O1),
        c.flux_eth.as_deref().map(Saved::FluxEth),
        c.bundles.as_deref().map(Saved::Bundles),
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
    receipt: &Receipt,
) -> GatewayResult<()> {
    if let Some(n)=&c.native_rfq{
        let nonce=c.chain.as_ref().ok_or_else(||ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,"Нет исходного nonce Native RFQ"))?.nonce;
        n.included(ctx,hash,nonce,receipt).await?;
    }
    if receipt.status.to::<u64>() != 1 {
        return Ok(());
    }
    for saved in entries(c) {
        match saved {
            Saved::SolidlyRouter(p) => p.included(ctx, hash, receipt).await?,
            Saved::Ranges(p)=>p.included(ctx,hash,receipt).await?,
            Saved::RegisteredV3(p) => p.included(ctx, hash, receipt).await?,
            Saved::AntfarmOutput(p) => p.included(ctx, hash, receipt).await?,
            Saved::Antfarm(p) => p.included(ctx, hash, receipt).await?,
            Saved::FluxEth(p)=>p.included(ctx,hash,receipt).await?,
            Saved::O1(p) => p.included(ctx, hash, receipt).await?,
            Saved::Bundles(p)=>p.included(ctx,hash,receipt).await?,
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
