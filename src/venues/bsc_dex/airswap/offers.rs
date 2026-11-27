//! Опрос настроенных маркетмейкеров по одному снимку и общему сроку ожидания.
use super::{
    Limits,
    client::{Maker, Query},
    no, profile, routing,
    signed::{prepare, signed},
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, parts::Parts, quote::white_state},
};
use alloy_primitives::Address;

pub(super) async fn options(
    p: &Parts,
    q: Query,
    limits: Limits,
    who: Address,
    head: white_state::Head,
) -> GatewayResult<Vec<routing::Ticket>> {
    let c = &p.ctx;
    let profiles = profile::Profile::candidates(c);
    let peers = Maker::endpoints(c);
    if peers.is_empty() {
        return Err(no(c, "AirSwap: не указан HTTPS-сервер maker"));
    }
    let generation = c.settings.generation();
    let mut out = Vec::new();
    let (di, dout) = tokio::try_join!(
        p.tokens.decimals(c, q.input),
        p.tokens.decimals(c, q.output)
    )?;
    let ms = c.int("airswap.min_validity_ms")?;
    use futures_util::{StreamExt, stream};
    let deadline = tokio::time::Instant::now() + c.timeout()?;
    let mut jobs = Vec::new();
    for scope in profiles {
        for maker in &peers {
            jobs.push(option(
                c,
                q,
                limits,
                head,
                (di, dout),
                scope,
                maker,
                (generation, who, ms),
            ));
        }
    }
    let mut jobs = stream::iter(jobs).buffer_unordered(4);
    let mut last = None;
    while let Ok(Some(result)) = tokio::time::timeout_at(deadline, jobs.next()).await {
        match result {
            Ok(mut rows) => out.append(&mut rows),
            Err(e) => last = Some(e),
        }
    }
    if c.settings.generation() != generation {
        return Err(no(c, "AirSwap: настройки изменились"));
    }
    if out.is_empty() {
        return Err(last.unwrap_or_else(|| {
            no(
                c,
                "AirSwap: нет оплачиваемой заявки в этих пределах и сроке",
            )
        }));
    }
    Ok(out)
}
#[allow(clippy::too_many_arguments)]
async fn option(
    c: &Ctx,
    q: Query,
    limits: Limits,
    head: white_state::Head,
    (di, dout): (u8, u8),
    scope: profile::Profile,
    maker: &Maker,
    (generation, who, ms): (u64, Address, u64),
) -> GatewayResult<Vec<routing::Ticket>> {
    let Some(prepared) = prepare(c, q, scope, maker, who, head, None).await? else {
        return Ok(vec![]);
    };
    signed(
        c,
        q,
        limits,
        head,
        (di, dout),
        scope,
        maker,
        (generation, who, ms),
        prepared,
    )
    .await
}
