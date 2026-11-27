//! Bound registry calls and creation events for each deployed LLAMMA factory generation.
use super::{
    balancer_v1::data,
    family_variant::Variant,
    llamma::{ILlama, Registry, Row, bad},
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, node::call},
};
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, Bytes, LogData, U256};

pub(super) fn creation_topic(variant: Variant) -> Option<B256> {
    match variant {
        Variant::Llamma(Registry::Mint) => Some(ILlama::AddMarket::SIGNATURE_HASH),
        Variant::Llamma(Registry::Lending | Registry::V2) => Some(ILlama::NewVault::SIGNATURE_HASH),
        _ => None,
    }
}

impl Registry {
    pub(super) fn created_market(self, log: &LogData) -> Option<(u64, Address)> {
        let (id, amm) = match self {
            Self::Mint => {
                let e = ILlama::AddMarket::decode_log_data(log).ok()?;
                (e.ix, e.amm)
            }
            Self::Lending | Self::V2 => {
                let e = ILlama::NewVault::decode_log_data(log).ok()?;
                (e.id, e.amm)
            }
        };
        Some((u64::try_from(id).ok()?, amm))
    }

    pub(super) fn count(self, f: Address) -> (Address, Bytes) {
        if self == Self::Mint {
            data(f, ILlama::n_collateralsCall {})
        } else {
            data(f, ILlama::market_countCall {})
        }
    }
    pub(super) fn ask(self, f: Address, index: u64) -> Vec<(Address, Bytes)> {
        let i = U256::from(index);
        match self {
            Self::V2 => vec![data(f, ILlama::marketsCall { i })],
            Self::Lending => vec![
                data(f, ILlama::ammsCall { i }),
                data(f, ILlama::controllersCall { i }),
                data(f, ILlama::borrowed_tokensCall { i }),
                data(f, ILlama::collateral_tokensCall { i }),
            ],
            Self::Mint => vec![
                data(f, ILlama::ammsCall { i }),
                data(f, ILlama::controllersCall { i }),
                data(f, ILlama::stablecoinCall {}),
                data(f, ILlama::collateralsCall { i }),
            ],
        }
    }
    pub(super) fn row(self, ctx: &Ctx, index: u64, answers: &[Bytes]) -> GatewayResult<Row> {
        let (amm, controller, coins) = match (self, answers) {
            (Self::V2, [tuple]) => {
                let v = call::decode::<ILlama::marketsCall>(tuple)?;
                (v.amm, v.controller, [v.borrowed, v.collateral])
            }
            (Self::Mint | Self::Lending, [a, b, c, d]) => (
                call::decode::<ILlama::ammsCall>(a)?,
                call::decode::<ILlama::controllersCall>(b)?,
                [
                    call::decode::<ILlama::borrowed_tokensCall>(c)?,
                    call::decode::<ILlama::collateral_tokensCall>(d)?,
                ],
            ),
            _ => return Err(bad(ctx)),
        };
        if amm.is_zero()
            || controller.is_zero()
            || coins.contains(&Address::ZERO)
            || coins[0] == coins[1]
        {
            return Err(bad(ctx));
        }
        Ok(Row {
            index,
            amm,
            controller,
            coins,
        })
    }
}
