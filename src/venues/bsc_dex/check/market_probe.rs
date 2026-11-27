//! Select both legs of a wallet probe on the same canonical market and its measured post-buy state.
use super::super::{
    ctx::Ctx,
    exec::{
        antfarm_wallet, carbon_direct, mint_club_binding, mint_club_direct,
        mint_club_wire::Protection, plan::Order,
    },
    pools::{antfarm_probe, antfarm_probe_wallet, registry::Entry, temple},
    quote::{
        ekubo_v2_pricing, vista_pricing,
        white_state::{self, Head},
    },
    wallet::approve::Approval,
};
use super::{
    carbon_probe, probe, probe_calls as pc,
    simulate::{Call, CallResult},
    vista_probe,
};
use crate::{
    core::types::PoolState,
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
};
use alloy_primitives::{Address, B256, Bytes, U256};

enum Venue {
    Regular,
    WalletRouter(Address),
    FundedExecutor,
    Temple,
    Antfarm(Option<Address>),
    Carbon,
    MintClub(Option<Protection>),
}
pub(super) struct Market {
    venue: Venue,
    anchor: Option<(Head, Box<Entry>)>,
    pub(super) vista: Option<Box<vista_probe::Probe>>,
}
pub(super) struct Leg {
    pub entry: Option<Box<Entry>>,
    pub call: (Vec<Approval>, Address, Bytes),
}
impl Market {
    pub async fn load(
        ctx: &Ctx,
        e: &Entry,
        number: u64,
        wallet: Address,
        amount: U256,
        source: Address,
    ) -> GatewayResult<Self> {
        let mut anchor = None;
        let mut vista = None;
        let venue = if mint_club_direct::enabled(e) {
            let head = white_state::head(ctx, &format!("{number:#x}")).await?;
            let state = mint_club_direct::state(ctx, e, head).await?;
            mint_club_direct::funded(ctx, wallet, e.usdt, amount, head).await?;
            let protection = if mint_club_binding::nominal(ctx, e, state.reserve, head)
                .await
                .is_ok()
            {
                None
            } else {
                Some(
                    mint_club_binding::module(
                        ctx,
                        e,
                        ctx.dex_addr("dex.router", e.family.id)?,
                        head,
                    )
                    .await?,
                )
            };
            let mut market = Box::new(e.clone());
            market.pool.state = PoolState::MintClub(Box::new(state));
            anchor = Some((head, market));
            Venue::MintClub(protection)
        } else if super::registered_v3::configured(ctx, e) {
            let head = white_state::head(ctx, &format!("{number:#x}")).await?;
            let executor = super::super::exec::executor::address(ctx).ok().flatten();
            match super::registered_v3::funded(ctx, e, wallet, amount, head).await {
                Err(_) if source != wallet && executor.is_some() => Venue::Regular,
                Err(err) => return Err(err),
                Ok(()) => {
                    anchor = Some((head, Box::new(e.clone())));
                    match super::registered_v3::router(ctx, e, head).await {
                        Ok(router) => Venue::WalletRouter(router),
                        Err(_) if executor.is_some() => Venue::FundedExecutor,
                        Err(err) => return Err(err),
                    }
                }
            }
        } else if antfarm_wallet::enabled(e) {
            anchor = Some(Box::pin(antfarm_probe::anchor(ctx, e, number)).await?);
            Venue::Antfarm(ctx.dex_addr("dex.vault",e.family.id).ok().filter(|v|!v.is_zero()))
        } else if matches!(&e.pool.state, PoolState::Temple(_)) {
            anchor = Some(Box::pin(temple::probe_state(ctx, e, number)).await?);
            Venue::Temple
        } else {
            let carbon = carbon_direct::enabled(e);
            if carbon
                || vista_pricing::native_router(e)
                || (ekubo_v2_pricing::enabled(e)
                    && matches!(&e.pool.state,PoolState::Ekubo(d) if d.native.is_some()))
            {
                vista = Box::pin(vista_probe::Probe::of(ctx, e, number, wallet))
                    .await?
                    .map(Box::new);
                if let Some(p) = &vista {
                    if carbon && let PoolState::Carbon(s) = p.market_state() {
                        Box::pin(carbon_probe::funded(ctx, s, e.usdt, amount, wallet, p.head))
                            .await?;
                    }
                    let mut market = Box::new(e.clone());
                    market.pool.state = p.state();
                    market.pool.block = number;
                    anchor = Some((p.head, market));
                }
            }
            if carbon {
                Venue::Carbon
            } else {
                Venue::Regular
            }
        };
        Ok(Self {
            venue,
            anchor,
            vista,
        })
    }
    pub fn entry<'a>(&'a self, original: &'a Entry) -> &'a Entry {
        self.anchor
            .as_ref()
            .map(|(_, e)| e.as_ref())
            .unwrap_or(original)
    }
    pub fn hash(&self) -> Option<B256> {
        self.anchor.as_ref().map(|(h, _)| h.hash)
    }
    fn self_funded(&self, e: &Entry, source: Address) -> bool {
        matches!(self.venue, Venue::Antfarm(vault) if antfarm_probe::uses_funding_pair(self.entry(e), source) || vault == Some(source))
    }
    pub fn funding(&self, e: &Entry, source: Address, wallet: Address, amount: U256) -> Vec<Call> {
        if matches!(
            self.venue,
            Venue::Carbon | Venue::MintClub(_) | Venue::WalletRouter(_) | Venue::FundedExecutor
        ) || self.self_funded(e, source)
        {
            Vec::new()
        } else {
            vec![pc::transfer(source, e.usdt, wallet, amount)]
        }
    }
    pub async fn leg(
        &self,
        ctx: &Ctx,
        e: &Entry,
        order: Order,
        wallet: Address,
        now: u64,
        prefix: &[Call],
    ) -> GatewayResult<Leg> {
        if matches!(self.venue, Venue::FundedExecutor) {
            let call = Box::pin(super::probe_exec::part(ctx, e, order, wallet, now)).await?;
            return Ok(Leg { entry: None, call });
        }
        if let Venue::WalletRouter(router) = self.venue {
            let call = super::registered_v3::leg(ctx, e, router, wallet, order, now)?;
            return Ok(Leg { entry: None, call });
        }
        if matches!(self.venue, Venue::Antfarm(_))
            && let Some((head, _)) = &self.anchor
        {
            let v = Box::pin(antfarm_probe_wallet::select(
                ctx, e, *head, wallet, &order, prefix,
            ))
            .await?;
            return Ok(Leg {
                entry: Some(v.entry),
                call: v.call,
            });
        }
        let protection = match self.venue {
            Venue::MintClub(p) => p,
            _ => None,
        };
        let call = Box::pin(probe::part(
            ctx,
            e,
            order,
            wallet,
            now,
            (self.vista.as_deref(), protection),
        ))
        .await?;
        Ok(Leg { entry: None, call })
    }
    pub fn observe(
        &self,
        ctx: &Ctx,
        e: &Entry,
        wallet: Address,
        list: &mut Vec<Call>,
    ) -> GatewayResult<Option<usize>> {
        if !matches!(self.venue, Venue::Antfarm(_)) {
            return Ok(None);
        }
        let (to, input) = antfarm_probe::request(ctx, e)?;
        let at = list.len();
        list.push(Call {
            from: wallet,
            to,
            input,
        });
        Ok(Some(at))
    }
    pub fn funding_error(
        &self,
        ctx: &Ctx,
        e: &Entry,
        source: Address,
        failure: (usize, &CallResult),
        observation: Option<usize>,
        quote: usize,
    ) -> Option<GatewayError> {
        let (i, r) = failure;
        (observation == Some(i) || (self.self_funded(e, source) && i < quote)).then(|| {
            ctx.err(
                ErrorCode::NoRoute,
                format!(
                    "Antfarm: проба не получила финансирование или состояние рынков: {}",
                    r.why()
                ),
            )
        })
    }
    pub fn after(
        &self,
        ctx: &Ctx,
        e: &Entry,
        wallet: Address,
        amounts: (U256, U256),
        buy: &CallResult,
        observed: Option<&CallResult>,
    ) -> GatewayResult<Option<Box<Entry>>> {
        let (amount, bought) = amounts;
        match self.venue {
            Venue::Antfarm(_) => {
                let r = observed.ok_or_else(|| {
                    ctx.err(
                        ErrorCode::BadResponse,
                        "Antfarm: отсутствует результат чтения после покупки",
                    )
                })?;
                antfarm_probe::after(ctx, e, &r.return_data).map(Some)
            }
            Venue::Carbon => carbon_probe::after(ctx, e, e.usdt, amount, &buy.logs),
            Venue::Temple => temple::after_probe(ctx, e, e.usdt, amount, &buy.logs),
            Venue::MintClub(p) => mint_club_direct::after_probe(
                ctx,
                e,
                e.usdt,
                amount,
                bought,
                (wallet, p, &buy.logs),
            ),
            Venue::Regular | Venue::WalletRouter(_) | Venue::FundedExecutor => Ok(None),
        }
    }
}
