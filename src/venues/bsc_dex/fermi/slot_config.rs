//! Параметры одного расчётного актива и разрешённого строителя с независимыми исполнителями площадок.
use super::super::{
    parts::Parts,
    pools::{families, family_variant::Variant, load_woo_ladder},
};
use super::{Builder, Deployment, prop_market::Oracle};
use alloy_primitives::{Address, B256};
use rust_decimal::Decimal;
#[derive(Clone)]
pub(super) enum Source {
    Fermi {
        runtime: B256,
    },
    Prop {
        id: &'static str,
        oracle: Oracle,
        pairs: Vec<Address>,
    },
}
#[derive(Clone)]
pub(super) struct Config {
    pub deployment: Deployment,
    pub executor: Address,
    pub source: Source,
    pub builder: Builder,
    pub interval: u64,
    pub timeout: u64,
    pub gas: Decimal,
    pub cash: Address,
    pub ladder: Vec<Decimal>,
}
impl Config {
    pub fn all(p: &Parts) -> Vec<Self> {
        Self::read(p).unwrap_or_default()
    }
    fn read(p: &Parts) -> Option<Vec<Self>> {
        let ctx = &p.ctx;
        if ctx.chain_id() != Some(1) || !ctx.flag("fermi.trading").ok()? {
            return None;
        }
        let builder = Builder::parse(&ctx.text("fermi.builder").ok()?)?;
        let positive = |key| ctx.int(key).ok().filter(|v| *v > 0);
        let interval = positive("fermi.books_interval_ms")?;
        let timeout = positive("fermi.quote_budget_ms")?;
        let gas = ctx
            .dec("fermi.gas_budget_native")
            .ok()
            .filter(|g| *g > Decimal::ZERO)?;
        let cash = p.tokens.usdt(ctx).ok()?;
        let ladder = load_woo_ladder::parse_sizes(
            ctx,
            "fermi.quote_sizes",
            &ctx.settings.list("fermi.quote_sizes", ctx.at()).ok()?,
        )
        .ok()?;
        if ladder.is_empty() {
            return None;
        }
        let build = |deployment, executor, source| Self {
            deployment,
            executor,
            source,
            builder,
            interval,
            timeout,
            gas,
            cash,
            ladder: ladder.clone(),
        };
        let mut out = Vec::new();
        if let (Ok(d), Ok(executor), Ok(runtime)) = (
            Deployment::read(ctx),
            ctx.addr("fermi.executor"),
            super::deployment::runtime(ctx, "fermi.executor_runtime_hash"),
        ) && !executor.is_zero()
        {
            out.push(build(d, executor, Source::Fermi { runtime }));
        }
        for f in families::all(ctx) {
            if f.variant != Variant::PropVenue || families::classify(ctx, f.id).is_err() {
                continue;
            }
            let profile = || {
                let market = ctx.dex_addr("dex.factory", f.id).ok()?;
                let executor = ctx
                    .dex_addr("dex.router", f.id)
                    .ok()
                    .filter(|a| !a.is_zero())?;
                let oracle =
                    Oracle::parse(market, &ctx.dex_list("dex.propamm_price_slots", f.id).ok()?)?;
                let mut pairs = Vec::new();
                for row in ctx.dex_list("dex.propamm_pairs", f.id).ok()? {
                    let (a, b) = row.split_once(' ')?;
                    let (a, b) = (
                        a.parse::<Address>().ok()?,
                        b.trim().parse::<Address>().ok()?,
                    );
                    let coin = if a == cash {
                        b
                    } else if b == cash {
                        a
                    } else {
                        continue;
                    };
                    if coin.is_zero() || coin == cash || pairs.contains(&coin) {
                        return None;
                    }
                    if p.tokens.ticker_of(coin).is_some() {
                        pairs.push(coin);
                    }
                }
                if pairs.is_empty() {
                    return None;
                }
                let d = Deployment {
                    router: market,
                    engine: oracle.address,
                    runtime: oracle.hash,
                };
                Some(build(
                    d,
                    executor,
                    Source::Prop {
                        id: f.id,
                        oracle,
                        pairs,
                    },
                ))
            };
            out.extend(profile());
        }
        Some(out)
    }
}

pub(super) fn timely(p: &Parts, b: &super::markets::Books) -> bool {
    p.ctx.settings.generation() == b.revision
        && b.current()
        && p.heads
            .fresh(&p.ctx)
            .is_ok_and(|h| h.number == b.head.number.to::<u64>())
}
pub(super) async fn fresh(p: &Parts, b: &super::markets::Books) -> bool {
    if !timely(p, b) {
        return false;
    }
    let (head, frame) = tokio::join!(
        super::rpc::call::<super::Head>(
            &p.ctx,
            "eth_getBlockByNumber",
            serde_json::json!(["latest", false])
        ),
        super::builder_snapshot(&p.ctx, b.config.builder)
    );
    let (Ok(h), Ok(raw)) = (head, frame) else {
        return false;
    };
    let new = match &b.config.source {
        Source::Fermi { .. } => super::Frame::read(b.config.builder, &raw, b.config.deployment),
        Source::Prop { oracle, .. } => oracle.frame(b.config.builder, &raw),
    };
    h.hash == b.head.hash
        && timely(p, b)
        && new.is_some_and(|f| {
            (f.target, f.slot, &f.diff) == (b.frame.target, b.frame.slot, &b.frame.diff)
        })
}
pub(super) async fn simulate(
    ctx: &super::Ctx,
    b: &super::markets::Books,
    proof: &super::slot::Proof,
    fees: (u128, u128),
) -> crate::gateway::errors::GatewayResult<(alloy_primitives::U256, alloy_primitives::U256, u64)> {
    if proof.prop.is_some() {
        super::prop_slot::simulate(ctx, &b.head, &b.frame, proof, Some(fees)).await
    } else {
        super::slot::simulate(ctx, &b.head, &b.frame, proof, Some(fees)).await
    }
}
