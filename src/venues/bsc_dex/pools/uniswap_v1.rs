//! Canonical registry and physical balances for Uniswap V1 (uniswap/v1-contracts, uniswap_{factory,exchange}.vy).
use super::{
    families::Family,
    nami_batch as batch,
    solidly_classic_factory::plain,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::{
        PoolRef,
        uniswap_v1::{UniswapV1State, V1Exchange},
    },
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{abi::IERC20, rpc},
    },
};
use alloy::sol;
use alloy_primitives::{Address, B256, U256};
use serde_json::{Value, json};

sol! {
    interface IFactory {
        function exchangeTemplate() external view returns(address);
        function getExchange(address token) external view returns(address);
        function getToken(address exchange) external view returns(address);
    }
    interface IExchange {
        function factoryAddress() external view returns(address);
        function tokenAddress() external view returns(address);
        function getEthToTokenInputPrice(uint256 amount) external view returns(uint256);
        function getTokenToEthInputPrice(uint256 amount) external view returns(uint256);
        function getEthToTokenOutputPrice(uint256 amount) external view returns(uint256);
        function getTokenToEthOutputPrice(uint256 amount) external view returns(uint256);
    }
}

fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Uniswap V1: фабрика, шаблон или стороны физической пары не подтверждены",
    )
}

async fn exchange(
    ctx: &Ctx,
    factory: Address,
    token: Address,
    pin: B256,
    at: &Value,
) -> GatewayResult<V1Exchange> {
    let f = IFactory::getExchangeCall { token };
    let [encoded] = batch::read(ctx, [batch::view(at, factory, f.clone())]).await?;
    let a = batch::decode(&encoded, f)?;
    if a.is_zero() {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Uniswap V1: у ERC20 нет биржи в этой фабрике",
        ));
    }
    let reverse = IFactory::getTokenCall { exchange: a };
    let cash = IERC20::balanceOfCall { owner: a };
    let facts = batch::read(
        ctx,
        [
            batch::view(at, factory, reverse.clone()),
            batch::view(at, a, IExchange::factoryAddressCall {}),
            batch::view(at, a, IExchange::tokenAddressCall {}),
            batch::view(at, token, cash.clone()),
        ],
    );
    let ((), raw, eth) = tokio::try_join!(
        plain(ctx, a, pin, at),
        facts,
        rpc::call::<U256>(ctx, "eth_getBalance", json!([a, at]))
    )?;
    // Original Vyper forwarders return a 4 KiB memory frame even for one ABI word.
    let address = |value: &alloy_primitives::Bytes| -> Option<Address> {
        if !matches!(value.len(), 32 | 4096) || value[..12] != [0; 12] {
            return None;
        }
        Some(Address::from_slice(&value[12..32]))
    };
    if batch::decode(&raw[0], reverse)? != token
        || address(&raw[1]) != Some(factory)
        || address(&raw[2]) != Some(token)
    {
        return Err(bad(ctx));
    }
    Ok(V1Exchange {
        address: a,
        token,
        eth,
        erc20: batch::decode(&raw[3], cash)?,
    })
}

pub(crate) async fn load(
    ctx: &Ctx,
    f: &Family,
    reference: &PoolRef,
    tokens: [Address; 2],
    at: &Value,
) -> GatewayResult<UniswapV1State> {
    let generation = ctx.settings.generation();
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let template = ctx.dex_addr("dex.v1_template", f.id)?;
    let pin = |key| {
        ctx.dex_text(key, f.id)?
            .parse::<B256>()
            .map_err(|_| bad(ctx))
    };
    let (a, b, c) = (
        pin("dex.factory_runtime_hash")?,
        pin("dex.v1_template_runtime_hash")?,
        pin("dex.pair_runtime_hash")?,
    );
    if tokens[0].is_zero() || tokens[0] >= tokens[1] || template == factory {
        return Err(bad(ctx));
    }
    let template_read = batch::read(
        ctx,
        [batch::view(at, factory, IFactory::exchangeTemplateCall {})],
    );
    let ((), (), [observed]) = tokio::try_join!(
        plain(ctx, factory, a, at),
        plain(ctx, template, b, at),
        template_read
    )?;
    if batch::decode(&observed, IFactory::exchangeTemplateCall {})? != template {
        return Err(bad(ctx));
    }
    let markets = match reference {
        PoolRef::Contract(expected) => {
            let wrapped = ctx.addr("dex.native_wrapped")?;
            let index = tokens
                .iter()
                .position(|t| *t == wrapped)
                .ok_or_else(|| bad(ctx))?;
            let asset = exchange(ctx, factory, tokens[1 - index], c, at).await?;
            if asset.address != *expected {
                return Err(bad(ctx));
            }
            let mut markets = [None, None];
            markets[1 - index] = Some(asset);
            markets
        }
        PoolRef::Multi {
            contract,
            token0,
            token1,
        } if *contract == factory && [*token0, *token1] == tokens => {
            let (l, r) = tokio::try_join!(
                exchange(ctx, factory, tokens[0], c, at),
                exchange(ctx, factory, tokens[1], c, at)
            )?;
            if l.address == r.address {
                return Err(bad(ctx));
            }
            [Some(l), Some(r)]
        }
        _ => return Err(bad(ctx)),
    };
    if generation != ctx.settings.generation() {
        return Err(bad(ctx));
    }
    Ok(UniswapV1State {
        factory,
        tokens,
        markets,
    })
}

pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let check: GatewayResult<PoolRef> = async {
        let pool = c.pool.parse::<Address>().map_err(|_| bad(ctx))?;
        let (token0, token1) = super::discover::sorted(c.coin, c.quote);
        let reference = if pool == ctx.dex_addr("dex.factory", c.family.id)? {
            PoolRef::Multi {
                contract: pool,
                token0,
                token1,
            }
        } else {
            PoolRef::Contract(pool)
        };
        let at = super::nami::block(ctx, "latest").await?;
        load(ctx, c.family, &reference, [token0, token1], &at).await?;
        Ok(reference)
    }
    .await;
    match check {
        Ok(r) => Verdict::Taken(Box::new(super::verify_woo::found(c, r))),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}

pub(super) fn topics() -> Vec<B256> {
    [
        "TokenPurchase(address,uint256,uint256)",
        "EthPurchase(address,uint256,uint256)",
        "AddLiquidity(address,uint256,uint256)",
        "RemoveLiquidity(address,uint256,uint256)",
    ]
    .map(alloy_primitives::keccak256)
    .to_vec()
}
