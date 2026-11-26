//! Снимок разрешённых книг Hanji. Книга, конфигурация и decimals относятся к одному хешу Base.
use super::{
    Level, Side, Units,
    live::{self, IHanji, Market},
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{ctx::Ctx, node::rpc, parts::Parts};
use alloy::{
    primitives::{Address, B256, U64},
    sol,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

sol! { function decimals() external view returns(uint8); }
#[derive(Clone, Deserialize)]
pub(super) struct Head {
    pub hash: B256,
    pub number: U64,
    pub timestamp: U64,
}
pub(super) async fn header(ctx: &Ctx) -> GatewayResult<Head> {
    let (id, head): (U64, Head) = tokio::try_join!(
        rpc::call(ctx, "eth_chainId", json!([])),
        rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false]))
    )?;
    if id != U64::from(8453) || ctx.chain_id() != Some(8453) {
        return Err(ctx.err(ErrorCode::NotApplicable, "Hanji: требуется узел Base"));
    }
    Ok(head)
}
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Config {
    pub executor: Address,
    pub cash: Address,
    pub gas: Decimal,
    pub timeout: Duration,
    pub age: Duration,
    pub deadline: u64,
    pub slip: alloy_primitives::U256,
    pub allowed: Vec<Address>,
}
impl Config {
    pub fn read(p: &Parts) -> Option<Self> {
        let c = &p.ctx;
        if c.chain_id() != Some(8453) || !c.flag("hanji.trading").unwrap_or(false) {
            return None;
        }
        let allowed = c
            .settings
            .list("hanji.markets", c.at())
            .ok()?
            .into_iter()
            .map(|s| s.parse::<Address>())
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        if allowed.is_empty() || allowed.iter().any(|a| a.is_zero()) {
            return None;
        }
        Some(Self {
            executor: c.addr("hanji.executor").ok().filter(|a| !a.is_zero())?,
            cash: p.tokens.usdt(c).ok()?,
            gas: c
                .dec("hanji.gas_budget_native")
                .ok()
                .filter(|v| *v > Decimal::ZERO)?,
            timeout: Duration::from_millis(c.int("hanji.quote_budget_ms").ok().filter(|v| *v > 0)?),
            age: Duration::from_millis(c.int("feed.stale_after_ms").ok().filter(|v| *v > 0)?),
            deadline: c.int("dex.deadline_s").ok().filter(|v| *v > 0)?,
            slip: crate::venues::bsc_dex::num::to_units(
                c.dec("exec.dex_slippage_pct")
                    .ok()?
                    .checked_div(Decimal::ONE_HUNDRED)?,
                18,
            )?,
            allowed,
        })
    }
}
pub(super) struct MarketBook {
    pub market: Address,
    pub x: Address,
    pub y: Address,
    pub units: Units,
    pub asks: Vec<Level>,
    pub bids: Vec<Level>,
    pub decimals: (u8, u8),
}
impl MarketBook {
    pub fn direction(&self, input: Address) -> Option<(Side, &[Level])> {
        if input == self.x {
            Some((Side::SellX, &self.bids))
        } else if input == self.y {
            Some((Side::BuyX, &self.asks))
        } else {
            None
        }
    }
}
pub(super) struct Books {
    pub head: Head,
    pub config: Config,
    pub by_coin: BTreeMap<Address, Vec<MarketBook>>,
    pub expires: i64,
    pub revision: u64,
}
async fn read(
    p: &Parts,
    m: &Market,
    cash: Address,
    h: B256,
) -> GatewayResult<(Address, MarketBook)> {
    use crate::venues::bsc_dex::node::multicall;
    use alloy::sol_types::SolCall;
    use alloy_primitives::Bytes;
    let ctx = &p.ctx;
    let x = m.base_token.contract_address;
    let y = m.quote_token.contract_address;
    let market = m.orderbook_address;
    let levels = |asks| IHanji::assembleOrderbookFromOrdersCall {
        market,
        asks,
        maxLevels: live::DEPTH.try_into().expect("uint24"),
    };
    let calls = [
        (market, Bytes::from(IHanji::getConfigCall {}.abi_encode())),
        (live::HELPER, Bytes::from(levels(true).abi_encode())),
        (live::HELPER, Bytes::from(levels(false).abi_encode())),
        (x, Bytes::from(decimalsCall {}.abi_encode())),
        (y, Bytes::from(decimalsCall {}.abi_encode())),
    ];
    let tag = json!({"blockHash":h,"requireCanonical":true});
    let invalid = || {
        ctx.err(
            ErrorCode::BadResponse,
            "Hanji: неполный пакет данных уровня, масштаба или комиссии",
        )
    };
    let values: Vec<Bytes> = if let Some(addr) = multicall::address(ctx) {
        let out: Bytes = rpc::call(
            ctx,
            "eth_call",
            json!([{"to":addr,"data":multicall::encode(&calls)},tag]),
        )
        .await?;
        multicall::decode(&out, calls.len())?
            .into_iter()
            .collect::<Result<_, _>>()
            .map_err(|_| invalid())?
    } else {
        let batch = calls
            .iter()
            .map(|(to, data)| ("eth_call", json!([{"to":to,"data":data},tag])))
            .collect::<Vec<_>>();
        rpc::batch(ctx, &batch)
            .await?
            .into_iter()
            .map(|v| serde_json::from_str(v?.get()).map_err(|_| invalid()))
            .collect::<GatewayResult<_>>()?
    };
    let [conf, ask, bid, dx, dy] = values.as_slice() else {
        return Err(invalid());
    };
    let cfg = IHanji::getConfigCall::abi_decode_returns(conf).map_err(|_| invalid())?;
    let decode_levels = |raw: &Bytes| -> GatewayResult<Vec<Level>> {
        let b = IHanji::assembleOrderbookFromOrdersCall::abi_decode_returns(raw)
            .map_err(|_| invalid())?;
        if b.prices.len() != b.shares.len() || b.prices.len() > live::DEPTH as usize {
            return Err(invalid());
        }
        Ok(b.prices
            .into_iter()
            .zip(b.shares)
            .map(|(p, shares)| Level {
                price: p.to(),
                shares,
            })
            .collect())
    };
    let asks = decode_levels(ask)?;
    let bids = decode_levels(bid)?;
    let dx = decimalsCall::abi_decode_returns(dx).map_err(|_| invalid())?;
    let dy = decimalsCall::abi_decode_returns(dy).map_err(|_| invalid())?;
    if cfg.x != x || cfg.y != y || x == y || x.is_zero() || y.is_zero() {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            "Hanji: токены в прокси и реестре расходятся",
        ));
    }
    let coin = if x == cash { y } else { x };
    p.tokens.remember_decimals(x, dx);
    p.tokens.remember_decimals(y, dy);
    Ok((
        coin,
        MarketBook {
            market,
            x,
            y,
            units: Units {
                x: cfg.scaleX,
                y: cfg.scaleY,
                fee_rate: u128::from(cfg.aggressiveFee) + u128::from(cfg.passivePayout),
            },
            asks,
            bids,
            decimals: if x == cash { (dy, dx) } else { (dx, dy) },
        },
    ))
}
pub(super) async fn load(p: &Parts, config: Config, markets: &[Market]) -> GatewayResult<Books> {
    let ctx = &p.ctx;
    let revision = ctx.settings.generation();
    let head = header(ctx).await?;
    super::wire::verify(ctx, &head, config.executor).await?;
    let matching = markets.iter().filter(|m| {
        let (x, y) = (
            m.base_token.contract_address,
            m.quote_token.contract_address,
        );
        config.allowed.contains(&m.orderbook_address)
            && (x == config.cash || y == config.cash)
            && p.tokens
                .ticker_of(if x == config.cash { y } else { x })
                .is_some()
    });
    let mut by_coin = BTreeMap::<Address, Vec<MarketBook>>::new();
    for (coin, b) in
        futures_util::future::join_all(matching.map(|m| read(p, m, config.cash, head.hash)))
            .await
            .into_iter()
            .flatten()
    {
        by_coin.entry(coin).or_default().push(b);
    }
    let expires =
        i64::try_from(u128::from(head.timestamp.to::<u64>()) * 1000 + config.age.as_millis())
            .unwrap_or(i64::MAX);
    if head.hash != header(ctx).await?.hash || revision != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Hanji: сменился блок или настройки во время чтения",
        ));
    }
    Ok(Books {
        head,
        config,
        by_coin,
        expires,
        revision,
    })
}
