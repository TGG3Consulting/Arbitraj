//! Лестницы точного входа отдельного PropAMM на опубликованном кадровом состоянии; встречная валюта определяется контрактом.
use super::super::{
    exec::{
        prop_abi::{IProp, IStelaxis},
        prop_wallet,
    },
    num,
    parts::Parts,
    pools::{families, prop_venue},
    quote::rate,
};
use super::{
    Ctx, Frame, Head, canonical, decimalsCall,
    markets::{Books, Config, SideBook},
    slot,
    slot_config::Source,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::primitives::{Address, Bytes, U64, U256, keccak256};
use alloy::sol_types::SolCall;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
pub(super) fn quote_call(
    router: Address,
    venue: Address,
    i: Address,
    o: Address,
    n: U256,
) -> Bytes {
    if router == venue {
        IStelaxis::quoteCall {
            input: i,
            output: o,
            amount: n,
        }
        .abi_encode()
        .into()
    } else {
        IProp::quoteVenueV1Call {
            venue,
            tokenIn: i,
            tokenOut: o,
            amountIn: n,
        }
        .abi_encode()
        .into()
    }
}
pub(super) fn quote_result(router: Address, venue: Address, raw: &[u8]) -> Option<U256> {
    let net = if router == venue {
        IStelaxis::quoteCall::abi_decode_returns(raw).ok()?
    } else {
        let q = IProp::quoteVenueV1Call::abi_decode_returns(raw).ok()?;
        if q.quotedVenue != venue {
            return None;
        }
        q.amountOut
    };
    (!net.is_zero()).then_some(net)
}
#[derive(Clone, Copy)]
pub(super) enum Quoted<'a> {
    Prop(prop_wallet::Terms),
    Bop(&'a super::bop_cash::Path),
}
impl<'a> Quoted<'a> {
    pub(super) fn book(book: &'a Books) -> Option<Self> {
        match &book.config.source {
            Source::Bop(path) => Some(Self::Bop(path)),
            _ => book.prop.map(Self::Prop),
        }
    }
}
pub(super) async fn quotes(
    ctx: &Ctx,
    h: &Head,
    f: &Frame,
    source: Quoted<'_>,
    i: Address,
    o: Address,
    sizes: &[U256],
) -> GatewayResult<Vec<(U256, U256)>> {
    let t = match source {
        Quoted::Bop(path) => return path.quotes(ctx, h, f, i, o, sizes).await,
        Quoted::Prop(t) => t,
    };
    let mut out = Vec::new();
    for group in sizes.chunks(16) {
        let calls: Vec<_> = group
            .iter()
            .map(|amount| {
                json!({"from":t.target,"to":t.router,"data":quote_call(t.router,t.venue,i,o,*amount)})
            })
            .collect();
        let rows = slot::run(ctx, h, f, json!(calls)).await?;
        if rows.len() != group.len() {
            return Err(ctx.err(ErrorCode::BadResponse, "Неполный ответ PropAMM в слоте"));
        }
        for (size, row) in group.iter().zip(rows) {
            if row.status == U64::from(1)
                && let Some(net) = quote_result(t.router, t.venue, &row.return_data)
            {
                out.push((*size, net));
            }
        }
    }
    Ok(out)
}
async fn read_pair(
    p: &Parts,
    h: &Head,
    f: &Frame,
    cfg: &Config,
    coin: Address,
    dc: u8,
) -> GatewayResult<(Option<prop_wallet::Terms>, SideBook, SideBook)> {
    let ctx = &p.ctx;
    let (source, terms, dec) = match &cfg.source {
        Source::Bop(path) => (
            Quoted::Bop(path),
            None,
            canonical(ctx, h.hash, coin, decimalsCall {}).await?,
        ),
        Source::Prop { id, .. } => {
            let family = families::classify(ctx, id)
                .map_err(|msg| ctx.err(ErrorCode::NotApplicable, msg))?;
            let at = json!({"blockHash":h.hash,"requireCanonical":true});
            let ((roles, enabled), dec) = tokio::try_join!(
                prop_venue::state(ctx, family, cfg.deployment.router, (cfg.cash, coin), &at),
                canonical(ctx, h.hash, coin, decimalsCall {})
            )?;
            if !enabled || !roles.accepts_frame(cfg.deployment.engine, cfg.deployment.runtime) {
                return Err(ctx.err(ErrorCode::NoRoute, "Именная площадка отключена"));
            }
            let t = super::prop_slot::terms(ctx, family, &roles, cfg.executor, h).await?;
            (Quoted::Prop(t), Some(t), dec)
        }
        _ => unreachable!(),
    };
    p.tokens.remember_decimals(coin, dec);
    let cash: Vec<_> = cfg
        .ladder
        .iter()
        .filter_map(|v| {
            num::to_units(*v, dc).filter(|n| !n.is_zero() && num::to_decimal(*n, dc) == Some(*v))
        })
        .collect();
    let buy = quotes(ctx, h, f, source, cfg.cash, coin, &cash).await?;
    let mut sells: BTreeSet<_> = cash
        .iter()
        .filter_map(|q| rate::from_usdt(ctx, &p.registry, cfg.cash, coin, *q).ok())
        .collect();
    sells.extend(buy.iter().map(|(_, q)| *q));
    sells.remove(&U256::ZERO);
    let sell = quotes(
        ctx,
        h,
        f,
        source,
        coin,
        cfg.cash,
        &sells.into_iter().collect::<Vec<_>>(),
    )
    .await?;
    let levels = |rows: Vec<(U256, U256)>, buy: bool| {
        let mut amounts = BTreeMap::new();
        for (i, o) in rows {
            let (qty, gross) = if buy { (o, i) } else { (i, o) };
            if let (Some(q), Some(c)) = (num::to_decimal(qty, dec), num::to_decimal(gross, dc)) {
                amounts
                    .entry(q)
                    .and_modify(|v: &mut rust_decimal::Decimal| {
                        *v = if buy { (*v).min(c) } else { (*v).max(c) }
                    })
                    .or_insert(c);
            }
        }
        SideBook(amounts.into_iter().collect())
    };
    Ok((terms, levels(buy, true), levels(sell, false)))
}
pub(super) async fn levels(
    p: &Parts,
    config: Config,
    head: Head,
    frame: Frame,
    revision: u64,
) -> GatewayResult<Books> {
    let ctx = &p.ctx;
    let bop_pair;
    let (pairs, dc): (&[Address], u8) = match &config.source {
        Source::Bop(path) => {
            bop_pair = [path.identity.cash.weth.address];
            let (_, dc) = tokio::try_join!(
                p.slot_markets
                    .bop_roles
                    .verify(ctx, head.hash, revision, *path),
                canonical(ctx, head.hash, config.cash, decimalsCall {})
            )?;
            (&bop_pair, dc)
        }
        Source::Prop { pairs, .. } => {
            let at = json!({"blockHash":head.hash,"requireCanonical":true});
            let (code, dc): (Bytes, u8) = tokio::try_join!(
                super::rpc::call(ctx, "eth_getCode", json!([frame.deployment.engine, at])),
                canonical(ctx, head.hash, config.cash, decimalsCall {})
            )?;
            if code.is_empty() || keccak256(code) != frame.deployment.runtime {
                return Err(ctx.err(ErrorCode::StaleData, "Изменился ценовой оракул"));
            }
            (pairs, dc)
        }
        _ => unreachable!(),
    };
    p.tokens.remember_decimals(config.cash, dc);
    let rows = futures_util::future::join_all(
        pairs
            .iter()
            .map(|c| read_pair(p, &head, &frame, &config, *c, dc)),
    )
    .await;
    let mut sides = BTreeMap::new();
    let mut prop = None;
    for (&c, r) in pairs.iter().zip(rows) {
        if let Ok((terms, buy, sell)) = r {
            prop = terms;
            if !buy.0.is_empty() {
                sides.insert((c, true), buy);
            }
            if !sell.0.is_empty() {
                sides.insert((c, false), sell);
            }
        }
    }
    let latest: Head =
        super::rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false])).await?;
    if head.hash != latest.hash {
        return Err(ctx.err(ErrorCode::StaleData, "Сменился родитель слота PropAMM"));
    }
    Ok(Books {
        config,
        head,
        frame,
        revision,
        prop,
        sides,
    })
}

#[cfg(test)]
mod stelaxis_tests {
    use super::*;
    use flate2::read::GzDecoder;
    use serde_json::Value;
    #[test]
    fn published_stelaxis_prices_use_the_original_quote_for_both_builders() {
        let data: Value = serde_json::from_reader(GzDecoder::new(
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/stelaxis_ethereum_frames.json.gz"
            )
            .as_slice(),
        ))
        .unwrap();
        let catalog: Vec<Value> =
            serde_json::from_str(include_str!("../../../../config/first_run_dex.json")).unwrap();
        let settings = &catalog
            .iter()
            .find(|p| p["id"] == "stelaxis-ethereum" && p["chain_id"] == 1)
            .unwrap()["settings"];
        let market: Address = settings["dex.factory"].as_str().unwrap().parse().unwrap();
        assert_eq!(settings["dex.factory"], settings["dex.quoter"]);
        let slots = settings["dex.propamm_price_slots"][0]
            .as_str()
            .unwrap()
            .split_whitespace()
            .skip(2)
            .collect::<BTreeSet<_>>();
        for f in data["frames"].as_array().unwrap() {
            assert_eq!(f["market"], settings["dex.factory"]);
            assert!(
                f["diff"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .all(|key| slots.contains(key.as_str()))
            );
        }
        let samples = data["samples"].as_array().unwrap();
        assert_eq!(samples.len(), 4);
        for r in samples {
            let addr = |k: &str| r[k].as_str().unwrap().parse::<Address>().unwrap();
            let n = U256::from_str_radix(r["amount"].as_str().unwrap(), 10).unwrap();
            assert_eq!(
                quote_call(market, market, addr("input"), addr("output"), n),
                r["data"].as_str().unwrap().parse::<Bytes>().unwrap()
            );
            let raw = r["result"].as_str().unwrap().parse::<Bytes>().unwrap();
            assert_eq!(
                quote_result(market, market, &raw),
                Some(U256::from_be_slice(&raw))
            );
            assert!(quote_result(Address::ZERO, market, &raw).is_none());
        }
    }
}
