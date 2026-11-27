//! Необязательный расчёт ордера в котировочной валюте через принятую пару V2.
//! Денежный предел проверяет RaindexCashWallet по конечным балансам, включая обратный обмен сдачи.
use super::*;
use crate::{
    settings::scope::Scope,
    venues::bsc_dex::{math::v2, pools::solidly_classic_factory as binding},
};
use alloy::sol_types::SolCall;
use alloy_primitives::keccak256;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
alloy::sol! {
    struct Request{address asset;uint256 cap;uint256 floor;uint256 bridgeLimit;uint256 deadline;bool buy;}
    function fill(Request r,bytes action) external;
    function getReserves() external view returns(uint112 r0,uint112 r1,uint32 t);
    function getAmountsIn(uint256 amount,address[] path) external view returns(uint256[]);
    function getAmountsOut(uint256 amount,address[] path) external view returns(uint256[]);
    event Settled(address indexed owner,bytes32 indexed action,address indexed asset,bool buy,uint256 spent,uint256 received,uint256 intermediate);
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Role {
    pub wrapper: Address,
    pub hash: B256,
    pub router: Address,
    pub pair: Address,
    pub cash: Address,
    pub bridge: Address,
}
#[derive(Clone, Copy)]
pub(super) struct State {
    pub role: Role,
    pub decimals: u8,
    reserves: [U256; 2],
    fee: (U256, U256),
}
#[derive(Clone, Copy)]
struct Entry {
    wrapper: Address,
    hash: B256,
    fee: (U256, U256),
}
async fn raw(ctx: &Ctx, h: white_state::Head, to: Address, input: Bytes) -> Option<Bytes> {
    rpc::call(
        ctx,
        "eth_call",
        json!([{"to":to,"input":input},h.selector()]),
    )
    .await
    .ok()
}
async fn word(ctx: &Ctx, h: white_state::Head, to: Address, name: &str) -> Option<B256> {
    let b = raw(ctx, h, to, Bytes::copy_from_slice(&keccak256(name)[..4])).await?;
    (b.len() == 32).then(|| B256::from_slice(&b))
}
async fn address(ctx: &Ctx, h: white_state::Head, to: Address, name: &str) -> Option<Address> {
    let b = word(ctx, h, to, name).await?;
    (b[..12].iter().all(|v| *v == 0))
        .then(|| Address::from_slice(&b[12..]))
        .filter(|a| !a.is_zero())
}
impl State {
    #[cfg(test)]
    pub(super) fn archived(role: Role, decimals: u8) -> Self {
        Self {
            role,
            decimals,
            reserves: [U256::ZERO; 2],
            fee: (U256::ONE, U256::ONE),
        }
    }

    pub(super) async fn configured(
        ctx: &Ctx,
        p: Profile,
        h: white_state::Head,
        cash: Address,
    ) -> Vec<Self> {
        let Some(rows) = ctx.settings.get_exact(
            "raindex.cash_routes",
            &Scope::Venue(ctx.place.id.to_string()),
        ) else {
            return vec![];
        };
        let mut entries = vec![];
        for row in rows
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
        {
            let words = row.split_whitespace().collect::<Vec<_>>();
            let [chain, book, wrapper, hash, fee] = words.as_slice() else {
                continue;
            };
            if chain.parse::<u64>().ok() != Some(p.chain)
                || book.parse::<Address>().ok() != Some(p.book)
            {
                continue;
            }
            if let (Ok(wrapper), Ok(hash), Some(fee)) = (
                wrapper.parse(),
                hash.parse(),
                fee.parse::<Decimal>().ok().and_then(v2::fee_ratio),
            ) && !entries.iter().any(|e: &Entry| e.wrapper == wrapper)
            {
                entries.push(Entry { wrapper, hash, fee });
            }
        }
        let mut out = vec![];
        for e in entries {
            if let Some(s) = Self::read(ctx, h, p, e, cash).await {
                out.push(s);
            }
        }
        out
    }
    async fn read(
        ctx: &Ctx,
        h: white_state::Head,
        p: Profile,
        e: Entry,
        cash: Address,
    ) -> Option<Self> {
        let w = e.wrapper;
        binding::plain(ctx, w, e.hash, &h.selector()).await.ok()?;
        let (book, r, pair, q, b, wh, rh, ph) = tokio::join!(
            address(ctx, h, w, "bookWallet()"),
            address(ctx, h, w, "router()"),
            address(ctx, h, w, "pair()"),
            address(ctx, h, w, "cash()"),
            address(ctx, h, w, "bridge()"),
            word(ctx, h, w, "walletCode()"),
            word(ctx, h, w, "routerCode()"),
            word(ctx, h, w, "pairCode()")
        );
        if book? != p.module || q? != cash || wh? != p.module_hash {
            return None;
        }
        let role = Role {
            wrapper: w,
            hash: e.hash,
            router: r?,
            pair: pair?,
            cash,
            bridge: b?,
        };
        let tag = h.selector();
        tokio::try_join!(
            binding::plain(ctx, role.router, rh?, &tag),
            binding::plain(ctx, role.pair, ph?, &tag)
        )
        .ok()?;
        let (a, b, res, dec) = tokio::join!(
            address(ctx, h, role.pair, "token0()"),
            address(ctx, h, role.pair, "token1()"),
            raw(ctx, h, role.pair, getReservesCall {}.abi_encode().into()),
            word(ctx, h, role.bridge, "decimals()")
        );
        let dec = u8::try_from(U256::from_be_bytes(dec?.0)).ok()?;
        let r = getReservesCall::abi_decode_returns_validate(&res?).ok()?;
        let mut reserves = [U256::from(r.r0), U256::from(r.r1)];
        let assets = [a?, b?];
        if assets == [role.bridge, role.cash] {
            reserves.swap(0, 1);
        } else if assets != [role.cash, role.bridge] {
            return None;
        }
        Some(Self {
            role,
            decimals: dec,
            reserves,
            fee: e.fee,
        })
    }
    pub(super) fn estimate(self, buy: bool, mid: U256) -> Option<U256> {
        let [cash, bridge] = self.reserves;
        if buy {
            v2::amount_in(mid, cash, bridge, self.fee)
        } else {
            v2::amount_out(mid, bridge, cash, self.fee)
        }
    }
    pub(super) async fn official(
        self,
        ctx: &Ctx,
        h: white_state::Head,
        buy: bool,
        mid: U256,
    ) -> Option<U256> {
        let r = self.role;
        let data = if buy {
            getAmountsInCall {
                amount: mid,
                path: vec![r.cash, r.bridge],
            }
            .abi_encode()
        } else {
            getAmountsOutCall {
                amount: mid,
                path: vec![r.bridge, r.cash],
            }
            .abi_encode()
        };
        let response = raw(ctx, h, r.router, data.into()).await?;
        let answer = if buy {
            getAmountsInCall::abi_decode_returns_validate(&response).ok()?
        } else {
            getAmountsOutCall::abi_decode_returns_validate(&response).ok()?
        };
        let [a, b] = answer.as_slice() else {
            return None;
        };
        if buy && *b == mid {
            Some(*a)
        } else if !buy && *a == mid {
            Some(*b)
        } else {
            None
        }
    }
}
