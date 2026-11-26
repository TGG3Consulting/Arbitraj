//! OasisSwap. Чистые резервы исключают feeCache. Комиссия определяется для
//! адреса получателя у штатного роутера и tx.origin у разрешённого адаптера V5.
use super::super::{
    ctx::Ctx,
    exec::executor,
    pools::{
        family_variant::Variant,
        oasis::{self, OasisMarket},
        registry::Entry,
    },
    wallet::signer,
};
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::{V5Step, Wire},
    white_state::Head,
};
use crate::{
    core::types::RouteMode,
    gateway::{errors::GatewayResult, quote_stage},
};
use alloy_primitives::{Address, Bytes};
pub(in crate::venues::bsc_dex) fn is_oasis(e: &Entry) -> bool {
    e.family.variant == Variant::Oasis
}

pub(in crate::venues::bsc_dex) async fn state(
    ctx: &Ctx,
    e: &Entry,
    wallet: Address,
    head: Head,
    adapter: bool,
) -> GatewayResult<OasisMarket> {
    oasis::at(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.pool.token0, e.pool.token1),
        &head.selector(),
        Some(wallet),
        adapter,
    )
    .await
}
pub(in crate::venues::bsc_dex) fn use_rebate(m: &OasisMarket, adapter: bool) -> bool {
    m.rebate_available && (adapter || m.native_rebate) && m.rebated < m.rate
}
pub(in crate::venues::bsc_dex) fn apply(e: &mut Entry, m: &OasisMarket, head: Head, rebate: bool) {
    e.pool.state = m.state();
    e.pool.fee_pips = Some((if rebate { m.rebated } else { m.rate }) * 100);
    e.pool.block = head.number.to();
}
pub(super) async fn candidates(q: &Quoting, i: &Input, head: Option<Head>) -> Vec<OptionQuote> {
    let ctx = &q.ctx;
    let Ok(wallet) = signer::address(ctx) else {
        return Vec::new();
    };
    let mut options = Vec::new();
    let graph =
        !quote_stage::settlement() && ctx.text("dex.router_mode").ok().as_deref() == Some("graph");
    for entry in i.entries.iter().filter(|e| is_oasis(e)) {
        for adapter in [false, true] {
            if adapter && (!graph || entry.family.executor_kind.is_none()) {
                continue;
            }
            let mut e = entry.clone();
            let (target, wire) = if let Some(h) = head {
                let Ok(m) = state(ctx, &e, wallet, h, adapter).await else {
                    continue;
                };
                let rebate = use_rebate(&m, adapter);
                if !adapter && rebate {
                    // Для малого объёма расход газа estimator может превысить сумму скидки.
                    let mut standard = e.clone();
                    apply(&mut standard, &m, h, false);
                    let exec = Execution {
                        mode: RouteMode::Direct,
                        target: m.native,
                        wire: Wire::Oasis { rebate: false },
                    };
                    if let Ok(v) = v2_execution::option(q, i, standard, wallet, exec, head).await {
                        options.push(v);
                    }
                }
                apply(&mut e, &m, h, rebate);
                if adapter {
                    let (Ok(Some(executor)), Some(module)) = (executor::address(ctx), m.adapter)
                    else {
                        continue;
                    };
                    (
                        executor,
                        Wire::V5(V5Step {
                            kind: 29,
                            pool: module,
                            data: Bytes::new(),
                        }),
                    )
                } else {
                    (m.native, Wire::Oasis { rebate })
                }
            } else if adapter {
                let (Ok(Some(executor)), Ok(module)) = (
                    executor::address(ctx),
                    ctx.dex_addr("dex.router", e.family.id),
                ) else {
                    continue;
                };
                (
                    executor,
                    Wire::V5(V5Step {
                        kind: 29,
                        pool: module,
                        data: Bytes::new(),
                    }),
                )
            } else {
                let Ok(router) = ctx.dex_addr("dex.quoter", e.family.id) else {
                    continue;
                };
                (router, Wire::Oasis { rebate: false })
            };
            if target.is_zero() {
                continue;
            }
            let mode = if adapter {
                RouteMode::Graph
            } else {
                RouteMode::Direct
            };
            if let Ok(c) =
                v2_execution::option(q, i, e, wallet, Execution { mode, target, wire }, head).await
            {
                options.push(c);
            }
        }
    }
    options
}
