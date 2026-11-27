//! Адрес разрешения прямого поручения выбирается тем же диспетчером, что и денежная отправка.
use super::super::{ctx::Ctx, pools::registry::Entry};
use super::market_plan::Market;
use crate::{
    core::types::RouterFeeMode,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::Address;

#[derive(Debug, PartialEq, Eq)]
enum Spender {
    Selected(Address),
    Factory,
    Missing,
}

fn selected(mode: Option<RouterFeeMode>) -> Spender {
    use RouterFeeMode::*;
    match mode {
        Some(EulerModuleV5{executor,..}|Llamma{v5:Some(executor),..}|YieldBasis{v5:Some(executor),..}|BancorLegacy{v5:Some(executor),..})=>Spender::Selected(executor),
        Some(
            NativeWallet { target, .. }
            | ProtectedWallet { target }
            | NativeFlat {target,..}
            | BancorLegacy {target,..}
            | Mooniswap {target,..}
            | DefiPlaza {target,..}
            | YieldBasis {target,..}
            | BalancerV1 {target,..}
            | BancorV3 {target,..}
            | UniswapV1 {target,..}
            | Llamma { target,v5:None,.. }
            | MotoswapPath {target,..}
            | FluidHop {target,..}
            | WalletBounds { target, .. }
            | Dagrow { target, .. }
            | AntfarmProceeds { target, .. },
        ) => Spender::Selected(target),
        _ => Spender::Missing,
    }
}

impl Market {
    fn approval(self, mode: Option<RouterFeeMode>) -> Spender {
        use RouterFeeMode::*;
        match self {
            Market::Ambient | Market::FluidDexLite => match mode {
                Some(FluidHop{target,..}) if matches!(self, Market::FluidDexLite)=>Spender::Selected(target),
                Some(WalletBounds {target,..}) => Spender::Selected(target),
                _ => Spender::Missing,
            },
            Market::Carbon => match mode {
                Some(CarbonWallet {
                    module: Some(target),
                    ..
                }) => Spender::Selected(target),
                _ => Spender::Factory,
            },
            Market::MintClub => match mode {
                Some(ProtectedWallet { target }) => Spender::Selected(target),
                _ => Spender::Factory,
            },
            Market::BancorLegacy | Market::Mooniswap | Market::DefiPlaza | Market::YieldBasis | Market::BalancerV1 | Market::BancorV3 | Market::UniswapV1 | Market::Rubicon | Market::NativeFlat=>selected(mode),
            Market::Prop=>match mode {Some(VenueFallback{target,..})=>Spender::Selected(target),_=>Spender::Missing},
            Market::Motoswap=>match mode{Some(MotoswapPath{target,..}|ProtectedWallet{target})=>Spender::Selected(target),_=>Spender::Missing},
            Market::Aqua | Market::Ammalgam | Market::Aktionariat | Market::Ranges | Market::SafeSwap | Market::Pendle => match mode {
                Some(ProtectedWallet { target }) => Spender::Selected(target),
                _ => Spender::Missing,
            },
            Market::RegisteredV3
            | Market::SolidlyRouter
            | Market::Umbrae
            | Market::O1
            | Market::Antfarm
            | Market::Angstrom
            | Market::EulerSwap | Market::Stabull
            | Market::Integral
            | Market::Llamma
            | Market::Dagrow
            | Market::Bundles
            | Market::Temple => selected(mode),
        }
    }
}
fn resolve(ctx: &Ctx, e: &Entry, spender: Spender) -> GatewayResult<Address> {
    match spender {
        Spender::Selected(target) => Ok(target),
        Spender::Factory => ctx.dex_addr("dex.factory", e.family.id),
        Spender::Missing => Err(ctx.err(
            ErrorCode::BadResponse,
            "В прямом маршруте отсутствует выбранный получатель разрешения",
        )),
    }
}
pub(in crate::venues::bsc_dex) fn spender(
    ctx: &Ctx,
    e: &Entry,
    mode: Option<RouterFeeMode>,
) -> Option<GatewayResult<Address>> {
    (matches!(mode,Some(RouterFeeMode::NativeFlat{..})).then_some(Market::NativeFlat).or_else(||Market::of(e))).map(|market| resolve(ctx, e, market.approval(mode)))
}
pub(in crate::venues::bsc_dex) fn signed(
    ctx: &Ctx,
    e: &Entry,
    mode: Option<RouterFeeMode>,
) -> GatewayResult<Address> {
    resolve(ctx, e, selected(mode))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_spender_and_factory_fallback_follow_the_original_market() {
        let target = Address::with_last_byte(9);
        let protected = Some(RouterFeeMode::ProtectedWallet { target });
        for m in [
            Market::Aqua,
            Market::Aktionariat,
            Market::RegisteredV3,
            Market::SolidlyRouter,
            Market::Umbrae,
            Market::O1,
            Market::Antfarm,
            Market::Dagrow,
            Market::Bundles,
            Market::Temple,
            Market::Pendle,
        ] {
            assert_eq!(m.approval(protected), Spender::Selected(target));
            assert_eq!(m.approval(None), Spender::Missing);
        }
        assert_eq!(
            Market::Pendle.approval(Some(RouterFeeMode::NativeWallet {
                target,
                native_cash: false
            })),
            Spender::Missing
        );
        assert_eq!(
            Market::MintClub.approval(protected),
            Spender::Selected(target)
        );
        assert_eq!(Market::MintClub.approval(None), Spender::Factory);
        assert_eq!(Market::Carbon.approval(None), Spender::Factory);
        assert_eq!(
            Market::Carbon.approval(Some(RouterFeeMode::CarbonWallet {
                module: Some(target),
                native_cash: false,
                allocation: None
            })),
            Spender::Selected(target)
        );
    }
}
