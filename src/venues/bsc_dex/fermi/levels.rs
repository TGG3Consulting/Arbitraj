//! Загружает однонаправленные котировки выбранного строителя для перестроения рыночного снимка. Использует только канонический Ethereum RPC.
use super::super::{num, parts::Parts};
use super::{
    Frame, Head, ROUTER, canonical, decimalsCall, getPairsCall,
    markets::{Books, Config, SideBook},
    quoteAmountsCall, rpc, slot,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::{
    primitives::{Address, Bytes, I256, U64},
    sol_types::SolCall,
};
use chrono::Utc;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
pub(super) async fn levels(p: &Parts, config: Config) -> GatewayResult<Books> {
    let ctx = &p.ctx;
    let (network, head, frame): (U64, Head, Frame) = tokio::try_join!(
        rpc::call(ctx, "eth_chainId", json!([])),
        rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false])),
        super::frame(ctx, config.builder)
    )?;
    let stale = || {
        ctx.err(
            ErrorCode::StaleData,
            "Fermi: сменился родитель или истёк кадр строителя",
        )
    };
    if network != U64::from(1)
        || !frame.current(
            head.number.to(),
            head.timestamp.to(),
            Utc::now().timestamp().max(0) as u64,
        )
    {
        return Err(stale());
    }
    let (_, _, pairs, dc) = tokio::try_join!(
        super::verify_router(ctx, &head),
        slot::verify_executor(ctx, &head, config.executor),
        canonical(ctx, head.hash, ROUTER, getPairsCall {}),
        canonical(ctx, head.hash, config.cash, decimalsCall {})
    )?;
    let coins: BTreeSet<_> = pairs
        .into_iter()
        .filter_map(|pair| {
            if pair.asset0 == config.cash {
                Some(pair.asset1)
            } else if pair.asset1 == config.cash {
                Some(pair.asset0)
            } else {
                None
            }
        })
        .filter(|a| !a.is_zero() && *a != config.cash && p.tokens.ticker_of(*a).is_some())
        .collect();
    let mut sizes = BTreeMap::new();
    for coin in coins {
        let dec = canonical(ctx, head.hash, coin, decimalsCall {}).await?;
        p.tokens.remember_decimals(coin, dec);
        sizes.insert(coin, dec);
    }
    p.tokens.remember_decimals(config.cash, dc);
    let mut requests = Vec::new();
    let mut args = Vec::new();
    for (&coin, &dec) in &sizes {
        for amount in &config.ladder {
            let Some(raw) = num::to_units(*amount, dc).filter(|r| {
                !r.is_zero() && r.bit_len() <= 255 && num::to_decimal(*r, dc) == Some(*amount)
            }) else {
                continue;
            };
            for buy in [true, false] {
                let (i, o) = if buy {
                    (config.cash, coin)
                } else {
                    (coin, config.cash)
                };
                let signed = I256::from_raw(raw);
                let call = quoteAmountsCall {
                    tokenIn: i,
                    tokenOut: o,
                    amountSpecified: if buy { signed } else { -signed },
                };
                requests.push(json!({"from":config.executor,"to":ROUTER,"data":Bytes::from(call.abi_encode())}));
                args.push((coin, dec, buy, raw, *amount));
            }
        }
    }
    let mut sides: BTreeMap<(Address, bool), SideBook> = BTreeMap::new();
    // Каждый пакет ограничен по числу вызовов, узел может вводить собственные лимиты eth_simulateV1.
    for (calls, keys) in requests.chunks(16).zip(args.chunks(16)) {
        let rows = slot::run(ctx, &head, &frame, json!(calls)).await?;
        if rows.len() != keys.len() {
            return Err(ctx.err(ErrorCode::BadResponse, "Fermi: неполный ответ с уровнями"));
        }
        for (r, &(coin, dec, buy, raw, cash)) in rows.iter().zip(keys) {
            if r.status != U64::from(1) {
                continue;
            }
            let Ok(q) = quoteAmountsCall::abi_decode_returns(&r.return_data) else {
                continue;
            };
            let (c, x) = if buy {
                (q.amountIn, q.amountOut)
            } else {
                (q.amountOut, q.amountIn)
            };
            if c != raw {
                continue;
            }
            if let Some(coin_qty) = num::to_decimal(x, dec).filter(|v| *v > Decimal::ZERO) {
                sides
                    .entry((coin, buy))
                    .or_default()
                    .0
                    .push((coin_qty, cash));
            }
        }
    }
    sides.retain(|_, levels| levels.0.windows(2).all(|w| w[0].0 < w[1].0));
    let latest: Head = rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false])).await?;
    let books = Books {
        config,
        revision: std::sync::atomic::AtomicU64::new(u64::MAX),
        head,
        frame,
        sides,
    };
    if latest.hash != books.head.hash || !books.current() {
        return Err(stale());
    }
    Ok(books)
}
