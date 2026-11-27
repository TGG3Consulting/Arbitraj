//! Исполнения Antfarm на одном состоянии, с одним и тем же объёмом входного ордера.
//! Каждый источник ATF проходит обычный расчёт обеих ног, затем замер выплаты и газа кошелька.
use super::super::{
    pools::{antfarm_book, antfarm_capacity, registry::Entry},
    wallet::signer,
};
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::Wire,
    white_state::Head,
};
use crate::core::types::{PoolState, RouteMode, Side};

pub(super) async fn options(
    q: &Quoting,
    i: &Input,
    entry: &Entry,
    head: Option<Head>,
) -> Vec<OptionQuote> {
    let ctx = &q.ctx;
    let Ok(payer) = signer::address(ctx) else {
        return Vec::new();
    };
    let mut e = entry.clone();
    if let Some(h) = head {
        let Ok(mut b) = antfarm_book::read(
            ctx,
            e.family,
            e.pool.reference.address(),
            (e.coin, e.usdt),
            h,
        )
        .await
        else {
            return Vec::new();
        };
        b.limits = antfarm_capacity::available(ctx, e.family, &b, h).await;
        e.pool.state = PoolState::Antfarm(Box::new(b));
        e.pool.block = h.number.to::<u64>();
    }
    let PoolState::Antfarm(book) = &e.pool.state else {
        return Vec::new();
    };
    let mut options = Vec::new();
    let input_module = if let Some(h) = head {
        antfarm_book::module(ctx, e.family, &h.selector())
            .await
            .ok()
    } else {
        ctx.dex_addr("dex.router", e.family.id).ok()
    };
    if let Some(target) = input_module {
        let mut input_only = e.clone();
        if let PoolState::Antfarm(b) = &mut input_only.pool.state {
            b.limits = None;
        }
        let wire = Wire::DirectModule {
            pool: e.pool.reference.address(),
        };
        if let Ok(v) = v2_execution::option(
            q,
            i,
            input_only,
            payer,
            Execution {
                mode: RouteMode::Direct,
                target,
                wire,
            },
            head,
        )
        .await
        {
            options.push(v);
        }
    }
    let Some(limits) = book.limits.filter(|l| l.payer == payer) else {
        return options;
    };
    let output = if i.side == Side::Buy { i.coin } else { i.usdt };
    let Some(index) = book.market.tokens.iter().position(|a| *a == output) else {
        return options;
    };
    for (fund, _) in &book.funding {
        if fund.tokens != [book.market.atf, output] {
            continue;
        }
        for available in [limits.stock, limits.loans[index]].into_iter().flatten() {
            let mut candidate = e.clone();
            let PoolState::Antfarm(b) = &mut candidate.pool.state else {
                continue;
            };
            b.proceeds = Some((fund.tier, available));
            let wire = Wire::DirectModule {
                pool: e.pool.reference.address(),
            };
            let execution = Execution {
                mode: RouteMode::Direct,
                target: limits.module,
                wire,
            };
            if let Ok(v) = v2_execution::option(q, i, candidate, payer, execution, head).await {
                options.push(v);
            }
        }
    }
    options
}
