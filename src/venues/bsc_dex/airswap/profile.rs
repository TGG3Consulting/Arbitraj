use super::{
    no,
    wire::{ISwap, IWallet, TYPE, Version},
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        ctx::Ctx, node::call, pools::solidly_classic_factory::plain, quote::white_state::Head,
    },
};
use alloy_primitives::{Address, B256, U256, keccak256};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Role {
    pub address: Address,
    pub hash: B256,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Profile {
    pub chain: u64,
    pub version: Version,
    pub exchange: Role,
    pub module: Option<Role>,
}
impl Profile {
    pub(super) fn parse(row: &str, chain: u64) -> Option<Self> {
        let mut it = row.split_whitespace();
        if it.next()?.parse::<u64>().ok()? != chain {
            return None;
        }
        let version = match it.next()? {
            "v4" => Version::V4,
            "v5" => Version::V5,
            _ => return None,
        };
        let role = |a: &str, h: &str| {
            let r = Role {
                address: a.parse().ok()?,
                hash: h.parse().ok()?,
            };
            (!r.address.is_zero() && !r.hash.is_zero()).then_some(r)
        };
        let exchange = role(it.next()?, it.next()?)?;
        let module = match it.next() {
            Some(a) => Some(role(a, it.next()?)?),
            None => None,
        };
        if it.next().is_some() || module.is_some_and(|m| m.address == exchange.address) {
            return None;
        }
        Some(Self {
            chain,
            version,
            exchange,
            module,
        })
    }
    pub(super) fn candidates(ctx: &Ctx) -> Vec<Self> {
        let Some(chain) = ctx.chain_id() else {
            return vec![];
        };
        let mut profiles = Vec::new();
        for row in ctx
            .settings
            .list("airswap.contracts", ctx.at())
            .unwrap_or_default()
        {
            if let Some(p) = Self::parse(&row, chain) {
                let direct = Self { module: None, ..p };
                if !profiles.contains(&direct) {
                    profiles.push(direct)
                }
                if p.module.is_some() && !profiles.contains(&p) {
                    profiles.push(p)
                }
            }
        }
        profiles
    }
    pub(super) fn target(self) -> Address {
        self.module.unwrap_or(self.exchange).address
    }
    pub(super) fn sender(self, owner: Address) -> Address {
        self.module.map_or(owner, |v| v.address)
    }
    pub(super) async fn check(
        &self,
        ctx: &Ctx,
        h: Head,
        owner: Address,
    ) -> GatewayResult<(B256, U256)> {
        let t = h.selector();
        let exchange = self.exchange.address;
        if ctx.chain_id() != Some(self.chain) {
            return Err(no(ctx, "AirSwap: другая сеть"));
        }
        plain(ctx, exchange, self.exchange.hash, &t).await?;
        let (domain, schema, fee) = tokio::try_join!(
            call::canonical(ctx, &t, exchange, ISwap::DOMAIN_SEPARATORCall {}),
            call::canonical(ctx, &t, exchange, ISwap::ORDER_TYPEHASHCall {}),
            call::canonical(ctx, &t, exchange, ISwap::protocolFeeCall {})
        )?;
        if domain.is_zero() || schema != keccak256(TYPE.as_bytes()) || fee >= U256::from(10000u16) {
            return Err(no(ctx, "AirSwap: неизвестная схема подписи или комиссия"));
        }
        if let Some(m) = self.module {
            let (_, who, sw, code) = tokio::try_join!(
                plain(ctx, m.address, m.hash, &t),
                call::canonical(ctx, &t, m.address, IWallet::ownerCall {}),
                call::canonical(ctx, &t, m.address, IWallet::exchangeCall {}),
                call::canonical(ctx, &t, m.address, IWallet::exchangeHashCall {})
            )?;
            if who != owner || sw != exchange || code != self.exchange.hash {
                return Err(no(ctx, "AirSwap: у модуля другой владелец или биржа"));
            }
        }
        Ok((domain, fee))
    }
}
