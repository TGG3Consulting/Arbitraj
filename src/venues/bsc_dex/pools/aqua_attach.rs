//! Историческая регистрация может давно быть отозвана. Пакет проверяет только токены подписанных пар.
use super::{
    aqua::{self, IAqua, Roles},
    aqua_book::{self, Book},
    created::Watch,
    families::Family,
    listen::Pools,
};
use crate::{gateway::errors::GatewayResult, venues::bsc_dex::quote::white_state::Head};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes};
use std::collections::{BTreeMap, BTreeSet};

pub(super) async fn connect(
    p: &Pools,
    watches: &[Watch],
    f: &'static Family,
    roles: Roles,
    book: &Book,
    hashes: &BTreeSet<B256>,
    tip: Head,
) -> GatewayResult<()> {
    let ctx = &p.ctx;
    let chain = ctx.chain_id().ok_or_else(|| aqua::bad(ctx))?;
    let mut jobs = BTreeSet::new();
    for &h in hashes {
        let Some(assets) = book.assets(h) else {
            continue;
        };
        for w in watches.iter().filter(|w| assets.contains(&w.usdt)) {
            for &token in assets
                .iter()
                .filter(|t| **t != w.usdt && w.coins.contains_key(*t))
            {
                jobs.insert((h, token));
                jobs.insert((h, w.usdt));
            }
        }
    }
    if jobs.is_empty() {
        return Ok(());
    }
    let jobs: Vec<_> = jobs.into_iter().collect();
    let mut calls = Vec::with_capacity(jobs.len());
    for &(order_hash, token) in &jobs {
        let maker = book.maker(order_hash).ok_or_else(|| aqua::bad(ctx))?;
        let call = IAqua::rawBalancesCall {
            maker,
            app: roles.engine,
            orderHash: order_hash,
            token,
        };
        calls.push((roles.registry, Bytes::from(call.abi_encode())));
    }
    let mut live = BTreeMap::<B256, Vec<Address>>::new();
    let values = super::balancer_v1::individual(ctx, &calls, &tip.selector()).await?;
    for ((h, t), r) in jobs.into_iter().zip(values) {
        let data = r.map_err(|_| aqua::bad(ctx))?;
        let answer = IAqua::rawBalancesCall::abi_decode_returns_validate(&data)
            .map_err(|_| aqua::bad(ctx))?;
        if (1..u8::MAX).contains(&answer.count) {
            live.entry(h).or_default().push(t);
        }
    }
    let mut active = Book::default();
    for (h, assets) in live.into_iter().filter(|(_, assets)| assets.len() >= 2) {
        book.copy_assets(h, &assets, &mut active);
    }
    aqua_book::publish(chain, roles, active.clone());
    for h in active.hashes() {
        for w in watches {
            super::created_log::aqua_attach(p, w, f, roles, &active, h)
                .await
                .map_err(|why| ctx.err(crate::gateway::errors::ErrorCode::BadResponse, why))?;
        }
    }
    Ok(())
}
