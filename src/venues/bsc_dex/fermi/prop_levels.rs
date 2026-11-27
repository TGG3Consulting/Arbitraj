//! Лестницы точного входа отдельного PropAMM на опубликованном кадровом состоянии; встречная валюта определяется контрактом.
use super::super::{
    exec::{prop_abi::IProp, prop_wallet},
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
pub(super) async fn quotes(
    ctx: &Ctx,
    h: &Head,
    f: &Frame,
    t: prop_wallet::Terms,
    i: Address,
    o: Address,
    sizes: &[U256],
) -> GatewayResult<Vec<(U256, U256)>> {
    let mut out = Vec::new();
    for group in sizes.chunks(16) {
        let calls: Vec<_> = group
            .iter()
            .map(|amount| {
                json!({"from":t.target,"to":t.router,"data":Bytes::from(IProp::quoteVenueV1Call{
            venue:t.venue,tokenIn:i,tokenOut:o,amountIn:*amount}.abi_encode())})
            })
            .collect();
        let rows = slot::run(ctx, h, f, json!(calls)).await?;
        if rows.len() != group.len() {
            return Err(ctx.err(ErrorCode::BadResponse, "Неполный ответ PropAMM в слоте"));
        }
        for (size, row) in group.iter().zip(rows) {
            if row.status == U64::from(1)
                && let Ok(q) = IProp::quoteVenueV1Call::abi_decode_returns(&row.return_data)
                && q.quotedVenue == t.venue
                && !q.amountOut.is_zero()
            {
                out.push((*size, q.amountOut));
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
) -> GatewayResult<(prop_wallet::Terms, SideBook, SideBook)> {
    let ctx = &p.ctx;
    let Source::Prop { id, .. } = &cfg.source else {
        unreachable!()
    };
    let family =
        families::classify(ctx, id).map_err(|msg| ctx.err(ErrorCode::NotApplicable, msg))?;
    let at = json!({"blockHash":h.hash,"requireCanonical":true});
    let ((roles, enabled), dec) = tokio::try_join!(
        prop_venue::state(ctx, family, cfg.deployment.router, (cfg.cash, coin), &at),
        canonical(ctx, h.hash, coin, decimalsCall {})
    )?;
    if !enabled {
        return Err(ctx.err(ErrorCode::NoRoute, "Именная площадка отключена"));
    }
    let terms = super::prop_slot::terms(ctx, family, &roles, cfg.executor, h).await?;
    p.tokens.remember_decimals(coin, dec);
    let cash: Vec<_> = cfg
        .ladder
        .iter()
        .filter_map(|v| {
            num::to_units(*v, dc).filter(|n| !n.is_zero() && num::to_decimal(*n, dc) == Some(*v))
        })
        .collect();
    let buy = quotes(ctx, h, f, terms, cfg.cash, coin, &cash).await?;
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
        terms,
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
    let Source::Prop { pairs, .. } = &config.source else {
        unreachable!()
    };
    let at = json!({"blockHash":head.hash,"requireCanonical":true});
    let (code, dc): (Bytes, u8) = tokio::try_join!(
        super::rpc::call(ctx, "eth_getCode", json!([frame.deployment.engine, at])),
        canonical(ctx, head.hash, config.cash, decimalsCall {})
    )?;
    if code.is_empty() || keccak256(code) != frame.deployment.runtime {
        return Err(ctx.err(ErrorCode::StaleData, "Изменился ценовой оракул"));
    }
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
            prop = Some(terms);
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
