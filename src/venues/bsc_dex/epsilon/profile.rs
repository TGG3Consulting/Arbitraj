//! Сетевой профиль Epsilon и доступность двух провайдеров по фактическим ролям в том же блоке.
use super::super::{
    ctx::Ctx, node::rpc, pools::solidly_classic_factory::plain, quote::white_state::Head,
};
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::json;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Contract {
    pub address: Address,
    pub code: B256,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Provider {
    pub family: String,
    pub contract: Contract,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Config {
    pub chain: u64,
    pub router: Contract,
    pub providers: [Provider; 2],
    pub wallet: Option<Contract>,
}
#[derive(Clone)]
pub(super) struct State {
    pub config: Config,
    pub fee: u16,
    pub factories: [Option<Address>; 2],
    pub guarded: bool,
}
impl Config {
    pub(super) fn read(ctx: &Ctx) -> Option<Self> {
        let rows = ctx.settings.list("epsilon.contracts", ctx.at()).ok()?;
        let mut router = None;
        let mut wallet = None;
        let mut providers = vec![];
        let chain = ctx.chain_id()?;
        for row in rows {
            let cols = row.split_whitespace().collect::<Vec<_>>();
            let (Some(id), Some(hash), Some(address)) = (
                cols.first(),
                cols.last(),
                cols.get(cols.len().checked_sub(2)?),
            ) else {
                return None;
            };
            if id.parse::<u64>().ok()? != chain {
                return None;
            }
            let c = Contract {
                address: address.parse().ok()?,
                code: hash.parse().ok()?,
            };
            if c.address.is_zero() || c.code.is_zero() || c.code == keccak256([]) {
                return None;
            }
            match cols.as_slice() {
                [_, "venue", _, _] if router.is_none() => router = Some(c),
                [_, "wallet", _, _] if wallet.is_none() => wallet = Some(c),
                [_, "provider", name, _, _] => providers.push(Provider {
                    family: (*name).into(),
                    contract: c,
                }),
                _ => return None,
            }
        }
        let providers: [Provider; 2] = providers.try_into().ok()?;
        let router = router?;
        if providers[0].family == providers[1].family
            || providers[0].contract.address == providers[1].contract.address
            || providers.iter().any(|p| {
                p.contract.address == router.address
                    || wallet.as_ref().is_some_and(|w| {
                        w.address == p.contract.address || w.address == router.address
                    })
            })
        {
            return None;
        }
        Some(Self {
            chain,
            router,
            providers,
            wallet,
        })
    }
    pub(super) async fn live(&self, ctx: &Ctx, h: Head) -> Option<State> {
        async fn word(ctx: &Ctx, h: Head, to: Address, sig: &str, arg: &[u8]) -> Option<B256> {
            let mut data = keccak256(sig)[..4].to_vec();
            data.extend_from_slice(arg);
            rpc::call(
                ctx,
                "eth_call",
                json!([{"to":to,"data":alloy_primitives::Bytes::from(data)},h.selector()]),
            )
            .await
            .ok()
        }
        let at = h.selector();
        let r = self.router.address;
        plain(ctx, r, self.router.code, &at).await.ok()?;
        let (paused, fee) = tokio::join!(
            word(ctx, h, r, "isPaused()", &[]),
            word(ctx, h, r, "protocolFeeBps()", &[])
        );
        if paused? != B256::ZERO {
            return None;
        }
        let fee = u16::try_from(U256::from_be_slice(fee?.as_slice())).ok()?;
        if fee >= 10000 {
            return None;
        }
        let mut factories = [None; 2];
        for (i, p) in self.providers.iter().enumerate() {
            let addr = p.contract.address;
            if plain(ctx, addr, p.contract.code, &at).await.is_ok()
                && word(
                    ctx,
                    h,
                    r,
                    "allowedTargets(address)",
                    addr.into_word().as_slice(),
                )
                .await
                    == Some(B256::from(U256::ONE))
                && let Some(w) = word(ctx, h, addr, "factory()", &[]).await
                && w[..12] == [0u8; 12]
            {
                let f = Address::from_slice(&w[12..]);
                if !f.is_zero() {
                    factories[i] = Some(f);
                }
            }
        }
        if factories.iter().all(Option::is_none) {
            return None;
        }
        let guarded = if let Some(w) = &self.wallet {
            plain(ctx, w.address, w.code, &at).await.is_ok()
                && word(ctx, h, w.address, "router()", &[]).await == Some(r.into_word())
                && word(ctx, h, w.address, "first()", &[]).await
                    == Some(self.providers[0].contract.address.into_word())
                && word(ctx, h, w.address, "second()", &[]).await
                    == Some(self.providers[1].contract.address.into_word())
        } else {
            false
        };
        Some(State {
            config: self.clone(),
            fee,
            factories,
            guarded,
        })
    }
}

impl State {
    pub(super) async fn accepts(
        &self,
        ctx: &Ctx,
        h: Head,
        order: &super::wire::Plan,
        families: [Option<&'static str>; 2],
    ) -> bool {
        let at = h.selector();
        if order.router != self.config.router.address
            || order.fee_bps != self.fee
            || order.wallet.is_some_and(|w| {
                !self.guarded || self.config.wallet.as_ref().is_none_or(|r| r.address != w)
            })
        {
            return false;
        }
        for (i, family) in families.into_iter().enumerate() {
            let Some(id) = family else { continue };
            if id != self.config.providers[i].family {
                return false;
            }
            let Some(factory) = self.factories[i] else {
                return false;
            };
            let mut data = keccak256("getPool(address,address,uint24)")[..4].to_vec();
            data.extend_from_slice(order.input.into_word().as_slice());
            data.extend_from_slice(order.output.into_word().as_slice());
            data.extend_from_slice(&U256::from(order.tiers[i]).to_be_bytes::<32>());
            let pool: Option<B256> = rpc::call(
                ctx,
                "eth_call",
                json!([{"to":factory,"input":alloy_primitives::Bytes::from(data)},at]),
            )
            .await
            .ok();
            if pool != Some(order.pools[i].into_word()) || order.pools[i].is_zero() {
                return false;
            }
            if order.wallet.is_none()
                && super::super::pools::exact_wallet_tokens::check(
                    ctx,
                    id,
                    order.input,
                    order.output,
                    &at,
                )
                .await
                .is_err()
            {
                return false;
            }
        }
        true
    }
}
