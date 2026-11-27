//! Размещение Ryze из настроек площадки. Адреса фабрики, пулов, счетов и код сверяются на одном каноническом блоке.
use super::super::{
    ctx::Ctx,
    node::{call::canonical, rpc},
    quote::white_state::Head,
};
use super::{http, wire};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::json;

alloy::sol! {
    function factory() external view returns(address);
    function pools(uint256 index) external view returns(address);
    function getPoolAsset(uint256 index) external view returns(address);
    function eip712Domain() external view returns(bytes1 fields,string name,string version,
        uint256 chainId,address verifyingContract,bytes32 salt,uint256[] extensions);
}
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    pub router: Address,
    pub router_hash: B256,
    pub factory: Address,
    pub factory_hash: B256,
    pub pool_hash: B256,
    pub tokens: Vec<super::tokens::Token>,
    pub pools: Vec<Address>,
    pub api: String,
    pub relayer: String,
    pub max_age_ms: u64,
    pub validity: u64,
    pub slippage: u16,
}

impl Config {
    pub fn read(ctx: &Ctx) -> Option<Self> {
        if ctx.chain_id() != Some(8453) || !ctx.flag("ryze.trading").unwrap_or(false) {
            return None;
        }
        let generation = ctx.settings.generation();
        let addr = |k| ctx.addr(k).ok().filter(|a| !a.is_zero());
        let hash = |k| {
            ctx.text(k)
                .ok()?
                .trim()
                .parse::<B256>()
                .ok()
                .filter(|h| !h.is_zero())
        };
        let pools: Vec<Address> = ctx
            .settings
            .list("ryze.pools", ctx.at())
            .ok()?
            .iter()
            .map(|s| s.trim().parse().ok())
            .collect::<Option<_>>()?;
        let tokens = ctx
            .settings
            .list("ryze.token_bindings", ctx.at())
            .ok()?
            .iter()
            .map(|s| super::tokens::Token::parse(s))
            .collect::<Option<Vec<_>>>()?;
        let api = ctx.text("ryze.quote_url").ok()?;
        let relayer = ctx.text("ryze.relayer_url").ok()?;
        http::endpoint(&api, "quote")?;
        http::endpoint(&relayer, "intents/submit")?;
        let c = Self {
            router: addr("ryze.router")?,
            router_hash: hash("ryze.router_runtime_hash")?,
            factory: addr("ryze.factory")?,
            factory_hash: hash("ryze.factory_runtime_hash")?,
            pool_hash: hash("ryze.pool_runtime_hash")?,
            tokens,
            pools,
            api,
            relayer,
            max_age_ms: ctx.int("ryze.quote_age_ms").ok()?,
            validity: ctx.int("ryze.validity_sec").ok()?,
            slippage: ctx.int("ryze.slippage_bps").ok()?.try_into().ok()?,
        };
        (generation == ctx.settings.generation()
            && c.router != c.factory
            && !c.tokens.is_empty()
            && c.tokens
                .iter()
                .enumerate()
                .all(|(i, t)| !c.tokens[..i].iter().any(|o| o.address == t.address))
            && !c.pools.is_empty()
            && c.pools.iter().enumerate().all(|(i, p)| {
                !p.is_zero() && *p != c.router && *p != c.factory && !c.pools[..i].contains(p)
            })
            && c.slippage < 10000
            && c.max_age_ms > 0
            && c.validity > 0)
            .then_some(c)
    }

    pub async fn verify(
        &self,
        ctx: &Ctx,
        h: Head,
    ) -> GatewayResult<(Vec<(Address, Address, Address)>, U256)> {
        let at = h.selector();
        let bad = || {
            ctx.err(
                ErrorCode::NotApplicable,
                "Ryze: изменились код, регистрация пула или домен подписи",
            )
        };
        let code = |a: Address| rpc::call::<Bytes>(ctx, "eth_getCode", json!([a, at]));
        let (router, factory, linked, domain, fee) = tokio::try_join!(
            code(self.router),
            code(self.factory),
            canonical(ctx, &at, self.router, factoryCall {}),
            canonical(ctx, &at, self.router, eip712DomainCall {}),
            canonical(ctx, &at, self.router, wire::intentFeeCall {})
        )?;
        if keccak256(router) != self.router_hash
            || keccak256(factory) != self.factory_hash
            || linked != self.factory
            || domain.fields.as_slice() != [15]
            || domain.name != "MultiHopRouter"
            || domain.version != "1"
            || domain.chainId != U256::from(8453)
            || domain.verifyingContract != self.router
            || !domain.salt.is_zero()
            || !domain.extensions.is_empty()
        {
            return Err(bad());
        }
        let mut markets = vec![];
        for (i, &pool) in self.pools.iter().enumerate() {
            let (address, pair, a, b) = tokio::try_join!(
                canonical(
                    ctx,
                    &at,
                    self.factory,
                    poolsCall {
                        index: U256::from(i)
                    }
                ),
                code(pool),
                canonical(ctx, &at, pool, getPoolAssetCall { index: U256::ZERO }),
                canonical(ctx, &at, pool, getPoolAssetCall { index: U256::ONE })
            )?;
            if address != pool
                || keccak256(pair) != self.pool_hash
                || a.is_zero()
                || b.is_zero()
                || a == b
            {
                return Err(bad());
            }
            markets.push((pool, a, b));
        }
        Ok((markets, fee))
    }
}
