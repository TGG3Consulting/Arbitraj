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
    Umbrae,
    Antfarm,
    Temple,
    Carbon,
    MintClub,
}

#[derive(Clone, Copy)]
pub(super) struct Selection {
    target: Option<Address>,
    carbon: Option<B256>,
    proceeds: Option<(u16, bool)>,
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
                proceeds: Some((tier, loan)),
            };
        }
        Self {
            target: super::mint_club_direct::selected(leg),
            carbon: super::carbon_direct::selected(leg).1,
            proceeds: None,
        }
    }
}
impl Market {
    pub(super) fn of(e: &Entry) -> Option<Self> {
        match &e.pool.state {
            _ if e.family.variant.umbrae() => Some(Self::Umbrae),
            PoolState::Antfarm(_) => Some(Self::Antfarm),
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
            (PoolState::Carbon(s), Self::Carbon) => s.native_side.is_some() == native,
            _ => !native,
        }
    }
    pub(super) fn missing_selection(self) -> &'static str {
        match self {
            Self::Umbrae => "Umbrae: требуется выбранный адрес исполнения",
            Self::Antfarm => "Antfarm: требуется подтверждённый бюджет комиссии",
            Self::Temple => "TempleDAO: требуется выбранный модуль денежного маршрута",
            Self::Carbon => "Carbon: требуется подтвержденный вектор действий контроллера",
            Self::MintClub => {
                "Mint Club: требуется прямое поручение с собственными границами суммы"
            }
        }
    }
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Umbrae => "Umbrae",
            Self::Antfarm => "Antfarm",
            Self::Temple => "TempleDAO",
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
        (_, Variant::Temple) => Some(super::temple_wallet::probe(ctx, e, order, wallet, now)),
        (PoolState::MintClub(_), _) => Some(super::mint_club_direct::probe(
            ctx, e, order, wallet, protection,
        )),
        (_, Variant::EkuboV2) => Some(super::ekubo_v2_direct::probe(ctx, e, order, now)),
        _ => None,
    }
}
