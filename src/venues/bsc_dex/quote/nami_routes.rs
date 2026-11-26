//! Сравнивает одиночный Nami с обычным сплитом. Его afterSwap может сам выбрать сторонние пулы.
use super::super::ctx::Ctx;
use super::{
    graph_split::{self, Cand},
    path::Path,
    route_calc::Leg,
};
use crate::core::types::TokenTaxes;
use crate::{
    core::types::Side,
    gateway::errors::{ErrorCode, GatewayResult},
};
use rust_decimal::Decimal;

pub(super) fn contains(legs: &[Leg]) -> bool {
    legs.iter()
        .any(|h| super::super::pools::nami::external_swap(&h.entry))
}
pub(super) fn solo(legs: &[Leg]) -> bool {
    legs.len() == 1 && contains(legs)
}

struct Choice {
    fill: Decimal,
    net: Decimal,
    span: std::ops::Range<usize>,
    paths: Vec<Path>,
}

pub(super) fn choose(
    ctx: &Ctx,
    cands: &mut [Cand],
    side: Side,
    qty: Decimal,
    taxes: &TokenTaxes,
    gas: bool,
) -> GatewayResult<Vec<Path>> {
    // Стабильный порядок между обычными кандидатами сохраняет прежние правила ничьей.
    cands.sort_by_key(|c| c.external_swap);
    let regular = cands.partition_point(|c| !c.external_swap);
    let groups = std::iter::once(0..regular)
        .filter(|s| !s.is_empty())
        .chain(
            (regular..cands.len())
                .filter(|i| solo(&cands[*i].legs))
                .map(|i| i..i + 1),
        )
        .collect::<Vec<_>>();
    let mut best: Option<Choice> = None;
    let mut reason = None;
    for span in groups {
        let group = &mut cands[span.clone()];
        match graph_split::ordinary(ctx, group, side, qty, taxes, gas) {
            Ok(paths) => {
                let fill = paths.iter().map(|p| p.coins).sum::<Decimal>().min(qty);
                let fees = if gas {
                    group
                        .iter()
                        .filter(|c| c.got.is_some())
                        .map(|c| c.gas_usdt)
                        .sum()
                } else {
                    Decimal::ZERO
                };
                let net = graph_split::score(side, paths.iter().map(|p| p.usdt).sum(), true, fees);
                if best.as_ref().is_none_or(|b| {
                    fill > b.fill || (fill == b.fill && graph_split::better(side, net, b.net))
                }) {
                    best = Some(Choice {
                        fill,
                        net,
                        span,
                        paths,
                    });
                }
            }
            Err(e) if e.code == ErrorCode::NoRoute => reason = Some(e),
            Err(e) => return Err(e),
        }
    }
    let best = best.ok_or_else(|| {
        reason.unwrap_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                "После учёта сторонних обменов Nami нет исполнимого пути",
            )
        })
    })?;
    for (i, cand) in cands.iter_mut().enumerate() {
        if !best.span.contains(&i) {
            cand.got = None;
            cand.qty = Decimal::ZERO;
        }
    }
    Ok(best.paths)
}
