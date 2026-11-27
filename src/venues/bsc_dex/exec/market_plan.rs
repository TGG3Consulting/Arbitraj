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
    Antfarm,
    Temple,
    Carbon,
    MintClub,
}

impl Market {
    pub(super) fn of(e: &Entry) -> Option<Self> {
        match &e.pool.state {
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
    pub(super) fn name(self) -> &'static str {
        match self {
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
        selected: (Option<Address>, Option<B256>),
    ) -> GatewayResult<Plan> {
        match self {
            Self::Antfarm => {
                Box::pin(super::antfarm_wallet::plan(
                    ctx, s, wallet, height, selected.0,
                ))
                .await
            }
            Self::Temple => {
                Box::pin(super::temple_wallet::plan(
                    ctx, s, wallet, height, selected.0,
                ))
                .await
            }
            Self::Carbon => {
                Box::pin(super::carbon_direct::path_plan(
                    ctx, s, h, wallet, height, buy, selected,
                ))
                .await
            }
            Self::MintClub => {
                Box::pin(super::mint_club_direct::path_plan(
                    ctx, s, h, wallet, height, selected.0,
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
        (PoolState::Antfarm(_), _) => Some(super::antfarm_wallet::probe(ctx, e, order, now)),
        (_, Variant::Temple) => Some(super::temple_wallet::probe(ctx, e, order, wallet, now)),
        (PoolState::MintClub(_), _) => Some(super::mint_club_direct::probe(
            ctx, e, order, wallet, protection,
        )),
        (_, Variant::EkuboV2) => Some(super::ekubo_v2_direct::probe(ctx, e, order, now)),
        _ => None,
    }
}
