//! Подписываемые вызовы одной площадки. Формы с собственным платежом сообщают ETH отдельно.
use alloy_primitives::{Address, Bytes, U256};

#[derive(Debug, Clone)]
pub(super) enum DirectCall {
    SolidlyRouter(std::sync::Arc<super::solidly_router_wallet::Proof>),
    Ranges(std::sync::Arc<super::ranges_wallet::Proof>),
    RegisteredV3(std::sync::Arc<super::registered_v3_wallet::Proof>),
    Carbon(std::sync::Arc<super::carbon_wire::Call>),
    O1(std::sync::Arc<super::o1_wallet::Proof>),
    FluxEth(std::sync::Arc<super::flux_eth::Proof>),
    Bundles(std::sync::Arc<super::bundles_wallet::Proof>),
    Aktionariat(std::sync::Arc<super::aktionariat_wallet::Proof>),
    BancorLegacy(std::sync::Arc<super::bancor_legacy_wallet::Proof>),
    Mooniswap(std::sync::Arc<super::mooniswap_wallet::Proof>),
    YieldBasis(std::sync::Arc<super::yield_basis_wallet::Proof>),
    BalancerV1(std::sync::Arc<super::balancer_v1_wallet::Proof>),
    BancorV3(std::sync::Arc<super::bancor_v3_wallet::Proof>),
    UniswapV1(std::sync::Arc<super::uniswap_v1_wallet::Proof>),
    Rubicon(std::sync::Arc<super::rubicon_wallet::Proof>),
    NativeFlat(std::sync::Arc<super::mars_wallet::Proof>),
    Prop(std::sync::Arc<super::prop_wallet::Proof>),
    Shiba(std::sync::Arc<super::shiba_payment::Proof>),
    Empire(std::sync::Arc<super::empire_wallet::Proof>),
    Angstrom(std::sync::Arc<super::angstrom_wallet::Proof>),
    Stabull(std::sync::Arc<super::stabull_wallet::Proof>),
    Integral(std::sync::Arc<super::integral_wallet::Proof>),
    Dagrow(std::sync::Arc<super::dagrow_wallet::Proof>),
    SafeSwap(std::sync::Arc<super::safeswap_wallet::Proof>),
    Pendle(std::sync::Arc<super::pendle_wallet::Proof>),
    Temple(std::sync::Arc<super::temple_wallet::Proof>),
    Antfarm(std::sync::Arc<super::antfarm_wallet::Proof>),
    AntfarmOutput(std::sync::Arc<super::antfarm_output_wallet::Proof>),
    MintClub(super::mint_club_wire::Call),
    EkuboV2 {
        key: super::super::pools::ekubo_v2::OrderKey,
        core: Address,
        wrapped: Option<Address>,
    },
    Module {
        pool: Address,
        input: Address,
        amount: U256,
    },
    Vista {
        call: super::vista_wire::Call,
        input: Address,
        amount: U256,
    },
    Oasis {
        input: Address,
        amount: U256,
        rebate: bool,
    },
    Vault {
        id: alloy_primitives::B256,
        input: Address,
        amount: U256,
    },
    Umbrae {
        path: super::umbrae::Path,
        input: Address,
        amount: U256,
    },
    SectorLb {
        path: super::sectorone::Path,
        input: Address,
        amount: U256,
    },
    V2SupportingFee {
        input: Address,
        amount: U256,
    },
}

impl DirectCall {
    pub(super) fn set_remeasured_minimum(&mut self, minimum: U256) -> Option<()> {
        match self {
            Self::Ranges(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::FluxEth(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::Bundles(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::Aktionariat(p)=>std::sync::Arc::make_mut(p).minimum=minimum,
            Self::BancorLegacy(p)=>std::sync::Arc::make_mut(p).minimum=minimum,
            Self::Mooniswap(p)=>std::sync::Arc::make_mut(p).minimum=minimum,
            Self::YieldBasis(p)=>std::sync::Arc::make_mut(p).minimum=minimum,
            Self::BalancerV1(p)=>std::sync::Arc::make_mut(p).minimum=minimum,
            Self::BancorV3(p)=>std::sync::Arc::make_mut(p).minimum=minimum,
            Self::UniswapV1(p)=>std::sync::Arc::make_mut(p).minimum=minimum,
            Self::Rubicon(p)=>std::sync::Arc::make_mut(p).minimum=minimum,
            Self::NativeFlat(p)=>std::sync::Arc::make_mut(p).minimum=minimum,
            Self::Prop(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::Shiba(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::Empire(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::Angstrom(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::Stabull(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::Integral(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::Dagrow(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::RegisteredV3(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::SolidlyRouter(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::O1(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::SafeSwap(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::Pendle(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::Temple(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::Antfarm(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::AntfarmOutput(p) => std::sync::Arc::make_mut(p).minimum = minimum,
            Self::Carbon(p) if p.by_target => {
                (U256::from(p.total()?) >= minimum).then_some(())?;
            }
            Self::Carbon(p) => std::sync::Arc::make_mut(p).limit = minimum.try_into().ok()?,
            Self::MintClub(c) if c.mint => {
                (c.qty >= minimum).then_some(())?;
            }
            Self::MintClub(c) => c.limit = minimum,
            // These ABIs take the current Plan minimum directly in data().
            Self::EkuboV2 { .. }
            | Self::Module { .. }
            | Self::Vista { .. }
            | Self::Oasis { .. }
            | Self::Vault { .. }
            | Self::Umbrae { .. }
            | Self::SectorLb { .. }
            | Self::V2SupportingFee { .. } => {}
        }
        Some(())
    }

    pub(super) fn native_value(self) -> Option<U256> {
        match self {
            Self::BancorLegacy(p)=>Some(p.terms.value(p.input,p.maximum)),
            Self::Mooniswap(p)=>Some(p.terms.value(p.input,p.maximum)),
            Self::YieldBasis(p)=>Some(p.terms.value(p.input,p.maximum)),
            Self::BalancerV1(p)=>Some(p.terms.value(p.input,p.maximum)),
            Self::BancorV3(p)=>Some(p.terms.value(p.maximum)),
            Self::UniswapV1(p)=>Some(p.terms.value(p.amount)),
            Self::NativeFlat(p)=>Some(p.market.wei),
            Self::FluxEth(p) => Some(p.value()),
            Self::Carbon(c) => Some(c.value()),
            Self::EkuboV2 { key, .. } => Some(key.value()),
            Self::Vista { call, amount, .. } => Some(call.value(amount)),
            _ => None,
        }
    }

    pub(super) fn data(
        self,
        output: Address,
        minimum: U256,
        wallet: Address,
        deadline: U256,
    ) -> Bytes {
        match self {
            Self::FluxEth(p) => p.data(),
            Self::Module {
                pool,
                input,
                amount,
            } => super::super::pools::solidly_v3::execution_data(
                pool, input, output, amount, minimum, deadline,
            ),
            Self::Vista {
                call,
                input,
                amount,
            } => call.data(input, output, amount, minimum, wallet, deadline),
            Self::Oasis {
                input,
                amount,
                rebate,
            } => super::oasis_direct::swap_data(
                input, output, amount, minimum, wallet, deadline, rebate,
            ),
            Self::Carbon(c) => c.data(),
            Self::O1(t) => t.data(),
            Self::Bundles(t) => t.data(),
            Self::Aktionariat(t)=>t.data(),
            Self::BancorLegacy(t)=>t.data(),
            Self::Mooniswap(t)=>t.data(),
            Self::YieldBasis(t)=>t.data(),
            Self::BalancerV1(t)=>t.data(),
            Self::BancorV3(t)=>t.data(),
            Self::UniswapV1(t)=>t.data(),
            Self::Rubicon(t)=>t.data(),
            Self::NativeFlat(t)=>t.data(),
            Self::Prop(t)=>t.data(),
            Self::Shiba(t) => t.data(),
            Self::Empire(t) => t.data(),
            Self::Angstrom(t) => t.data(),
            Self::Stabull(t) => t.data(),
            Self::Integral(t) => t.data(),
            Self::Dagrow(t) => t.data(),
            Self::SafeSwap(t) => t.data(),
            Self::Pendle(t) => t.data(),
            Self::Temple(t) => t.data(),
            Self::Antfarm(a) => a.data(),
            Self::AntfarmOutput(a) => a.data(),
            Self::MintClub(order) => order.data(wallet),
            Self::EkuboV2 { key, .. } => key.calldata(minimum, deadline),
            Self::Vault { id, input, amount } => {
                super::vault_direct::data(id, input, output, amount, minimum, wallet, deadline)
            }
            Self::SolidlyRouter(proof) => proof.data(),
            Self::Ranges(proof) => proof.data(),
            Self::RegisteredV3(proof) => proof.data(),
            Self::Umbrae {
                path,
                input,
                amount,
            } => path.data(input, output, amount, minimum, wallet, deadline),
            Self::SectorLb {
                path,
                input,
                amount,
            } => path.data(input, output, amount, minimum, wallet, deadline),
            Self::V2SupportingFee { input, amount } => {
                super::fork_swap::swap_data(amount, minimum, &[input, output], wallet, deadline)
            }
        }
    }
}
