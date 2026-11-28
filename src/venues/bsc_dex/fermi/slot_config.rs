//! Параметры одного расчётного актива и разрешённого строителя с независимыми исполнителями площадок.
//! Лента книг и подготовка заявок читают настройки Ethereum и отдельные разрешения каждого рынка.
use super::super::{
    parts::Parts,
    pools::{families, load_woo_ladder},
};
pub(super) use super::slot_checks::{fresh, same_frame, simulate, timely};
use super::{Builder, Deployment, prop_oracle::Oracle, prop_profile};
use alloy_primitives::{Address, B256};
use rust_decimal::Decimal;
#[derive(Clone)]
pub(super) enum Source {
    Bop(super::bop_cash::Path),
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
        if ctx.chain_id() != Some(crate::settings::evm_profiles::ETHEREUM_MAINNET) {
            return None;
        }
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
        let build = |builder, deployment, executor, source| Self {
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
        if ctx.flag("fermi.trading").unwrap_or(false)
            && let Ok(name) = ctx.text("fermi.builder")
            && let Some(builder) = Builder::parse(&name)
            && let (Ok(d), Ok(executor), Ok(runtime)) = (
                Deployment::read(ctx),
                ctx.addr("fermi.executor"),
                super::deployment::runtime(ctx, "fermi.executor_runtime_hash"),
            )
            && !executor.is_zero()
        {
            out.push(build(builder, d, executor, Source::Fermi { runtime }));
        }
        for (builder, path) in super::bop_cash::configured(ctx, cash) {
            let profile = path.profile;
            let deployment = Deployment {
                router: profile.market.address,
                engine: profile.registry.address,
                runtime: profile.market.hash,
            };
            out.push(build(
                builder,
                deployment,
                path.module.address,
                Source::Bop(path),
            ));
        }
        if prop_profile::network(ctx).is_none() {
            return Some(out);
        }
        for f in families::all(ctx)
            .into_iter()
            .filter(|f| f.variant.prop_venue())
        {
            let Some(market_profile) = prop_profile::Profile::read(ctx, f.id) else {
                continue;
            };
            let permitted = market_profile.builders;
            let profile = || {
                let oracle = market_profile.oracle;
                let market = oracle.market;
                let executor = ctx
                    .dex_addr("dex.router", f.id)
                    .ok()
                    .filter(|a| !a.is_zero())?;
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
                Some((
                    d,
                    executor,
                    Source::Prop {
                        id: f.id,
                        oracle,
                        pairs,
                    },
                ))
            };
            if let Some((deployment, executor, source)) = profile() {
                out.extend(
                    permitted
                        .into_iter()
                        .map(|builder| build(builder, deployment, executor, source.clone())),
                );
            }
        }
        Some(out)
    }
}

pub(super) fn builders(rows: Vec<String>) -> Vec<Builder> {
    let mut out = Vec::new();
    for row in rows {
        let Some(b) = Builder::parse(row.trim()) else {
            return Vec::new();
        };
        if out.contains(&b) {
            return Vec::new();
        }
        out.push(b);
    }
    out
}
impl Config {
    pub(super) fn same_path(&self, other: &Self) -> bool {
        match (&self.source, &other.source) {
            (Source::Bop(a), Source::Bop(b)) => a.bridge == b.bridge,
            (Source::Bop(_), _) | (_, Source::Bop(_)) => false,
            _ => true,
        }
    }
    pub(super) fn permitted(&self, ctx: &super::Ctx) -> bool {
        match &self.source {
            Source::Bop(_) => super::bop_cash::allowed(ctx).contains(&self.builder),
            Source::Fermi { .. } => {
                ctx.flag("fermi.trading").unwrap_or(false)
                    && ctx.text("fermi.builder").as_deref().ok() == Some(self.builder.name())
            }
            Source::Prop { id, .. } => {
                ctx.dex_flag("dex.pool_enabled", id).unwrap_or(false)
                    && builders(
                        ctx.dex_list("dex.propamm_slot_builders", id)
                            .unwrap_or_default(),
                    )
                    .contains(&self.builder)
            }
        }
    }
}
