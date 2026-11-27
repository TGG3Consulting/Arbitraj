//! Полный канонический интервал допуска подписи; последняя секунда срока включена.
use super::{Terms, cash::Fill, ledger::Marker};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::rpc,
        quote::white_state::{self, Head},
        send,
    },
};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, B256, U64, U256};
use serde::Deserialize;
use serde_json::{Value, json};
alloy::sol! {
    event OrderCancelled(address indexed swapper,bytes32 indexed orderHash,bool success);
    event UnorderedNonceInvalidation(address indexed owner,uint256 word,uint256 mask);
}
#[derive(Default)]
pub(super) struct Events {
    pub original: Option<B256>,
    pub excludes: bool,
}
pub(super) fn fill_filter(who: Address) -> Value {
    json!([Fill::SIGNATURE_HASH, null, null, who.into_word()])
}
pub(super) fn expired(timestamp: u64, deadline: u64) -> bool {
    timestamp > deadline
}
async fn last(ctx: &Ctx, m: &Marker, t: &Terms, tip: Head) -> GatewayResult<u64> {
    let origin = white_state::head(ctx, &format!("{:#x}", m.start)).await?;
    let mut high = tip.number.to::<u64>();
    let mut low = m.start;
    if origin.hash != m.anchor || low > high || expired(origin.timestamp.to(), t.deadline) {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "PCSX: изменился исходный финализированный блок",
        ));
    }
    if !expired(tip.timestamp.to(), t.deadline) {
        return Ok(high);
    }
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let h = white_state::head(ctx, &format!("{mid:#x}")).await?;
        if expired(h.timestamp.to(), t.deadline) {
            high = mid - 1;
        } else {
            low = mid;
        }
    }
    Ok(low)
}
pub(super) async fn scan(
    ctx: &Ctx,
    m: &Marker,
    t: &Terms,
    tip: Head,
) -> GatewayResult<(Head, Events)> {
    let mut start = m.start;
    let end = last(ctx, m, t, tip).await?;
    let cutoff = white_state::head(ctx, &format!("{end:#x}")).await?;
    let p = &m.ticket.cfg.p;
    let who = m.ticket.req.owner;
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "PCSX: квитанция или событие вне канонической области поиска",
        )
    };
    let mut out = Events::default();
    while start <= end {
        let hi = start.saturating_add(999).min(end);
        let query = |addr: Address, topics: Value| {
            rpc::call::<Vec<Log>>(
                ctx,
                "eth_getLogs",
                json!([{"address":addr,"topics":topics,"fromBlock":format!("{start:#x}"),"toBlock":format!("{hi:#x}")}]),
            )
        };
        let (fills, cancels, permits) = tokio::try_join!(
            query(p.router.address, fill_filter(who)),
            query(
                p.router.address,
                json!([
                    OrderCancelled::SIGNATURE_HASH,
                    who.into_word(),
                    t.order_hash
                ])
            ),
            query(
                p.permit.address,
                json!([UnorderedNonceInvalidation::SIGNATURE_HASH, who.into_word()])
            )
        )?;
        for log in fills.into_iter().chain(cancels).chain(permits) {
            if log.removed || log.block_number.is_none_or(|n| n < start || n > hi) {
                return Err(bad());
            }
            let id = log.transaction_hash.ok_or_else(bad)?;
            let own = if log.topics().first() == Some(&Fill::SIGNATURE_HASH) {
                let a = Fill::decode_log_data(&log.inner.data).map_err(|_| bad())?;
                if log.address() != p.router.address || a.swapper != who {
                    return Err(bad());
                }
                if a.nonce != t.nonce {
                    continue;
                }
                a.orderHash == t.order_hash
            } else if log.topics().first() == Some(&OrderCancelled::SIGNATURE_HASH) {
                let a = OrderCancelled::decode_log_data(&log.inner.data).map_err(|_| bad())?;
                if log.address() != p.router.address
                    || a.swapper != who
                    || a.orderHash != t.order_hash
                {
                    return Err(bad());
                }
                if !a.success {
                    continue;
                }
                false
            } else {
                let a = UnorderedNonceInvalidation::decode_log_data(&log.inner.data)
                    .map_err(|_| bad())?;
                if log.address() != p.permit.address || a.owner != who {
                    return Err(bad());
                }
                if a.word != t.nonce >> 8 || !a.mask.bit((t.nonce & U256::from(255)).to::<usize>())
                {
                    continue;
                }
                false
            };
            let receipt = send::receipt::read(ctx, id).await?.ok_or_else(bad)?;
            if receipt.status != U64::from(1)
                || receipt.block_hash != log.block_hash
                || !receipt.logs.iter().any(|r| {
                    r.inner == log.inner
                        && r.log_index == log.log_index
                        && r.transaction_hash == Some(id)
                })
            {
                return Err(bad());
            }
            send::canonical_receipt::verify(ctx, id, &receipt).await?;
            let h = white_state::head(ctx, &format!("{:#x}", receipt.block_number)).await?;
            if Some(h.hash) != receipt.block_hash {
                return Err(bad());
            }
            p.check(ctx, h, &m.ticket.offer.version).await?;
            if own {
                if out.original.replace(id).is_some() {
                    return Err(bad());
                }
            } else {
                out.excludes = true;
            }
        }
        if hi == end {
            break;
        }
        start = hi + 1;
    }
    if white_state::head(ctx, &format!("{:#x}", tip.number))
        .await?
        .hash
        != tip.hash
    {
        return Err(bad());
    }
    Ok((cutoff, out))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Transaction {
    pub hash: B256,
    pub from: Address,
    pub block_hash: B256,
    pub nonce: U64,
}
