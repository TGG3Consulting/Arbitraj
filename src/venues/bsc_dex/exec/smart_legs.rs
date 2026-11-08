//! Файл: venues/bsc_dex/exec/smart_legs.rs
//! Что делает: подготовка долей умного маршрута (решение 38) по живому состоянию: количество каждой доли (продажа —
//! qty × доля входа; покупка — qty × ожидаемые монеты доли / все; последняя — остаток), путь доли (exec/live.rs),
//! проверка средней цены против худшей допустимой, сверка с MixedQuoter PancakeSwap, шаги и минимумы (exec/chain.rs),
//! свежие минимумы графовой верификации (решение 83) последнему шагу каждой доли, нативная обёртка (решение 62).
//! Зовётся дважды, если отправлялись разрешения (решение 121 B1: после approve всё заново).
//! Решение 223: доли графа с общим пулом — суммы, цена и шаги по порядку исполнения на последовательных состояниях
//! (quote/graph_seq.rs resequence); MixedQuoter сверяет каждую долю на текущем состоянии — с расчётом доли на нём же.
//! Кто вызывает: exec/trade_smart.rs.
//! От чего зависит: exec/live.rs, exec/chain.rs, exec/native.rs, exec/order.rs (check_price), exec/verify_gate.rs,
//! quote/mixed_quoter.rs, quote/graph_seq.rs, quote/via.rs, quote/path.rs.
//! Настройки: через вызываемые — smart.mixed_quoter, dex.quoter_max_diff_pct, exec.dex_slippage_pct, dex.*.

use rust_decimal::Decimal;

use crate::core::types::{DexRoute, Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::quote::graph_seq;
use super::super::quote::mixed_quoter::{self, Leg};
use super::super::quote::path::Path;
use super::super::quote::via::Via;
use super::chain::Step;
use super::native::{self, Wrap};
use super::order::check_price;
use super::{chain, live, verify_gate};

/// Подготовленные доли: пути по живому состоянию (с количеством доли), шаги с минимумами, нативная обёртка.
pub struct Prep {
    pub paths: Vec<(Path, Decimal)>,
    pub steps: Vec<Vec<Step>>,
    pub wrap: Wrap,
}

/// Доли по живому состоянию: количество каждой доли, путь (exec/live.rs), проверка средней цены против худшей, сверка
/// с MixedQuoter, шаги и минимумы (exec/chain.rs), свежие минимумы верификации, нативная обёртка.
/// Принимает: расчёт путей, запрос, маршрут, минимумы верификации, decimals, нативная ли монета.
/// Ошибки: NoRoute (нет долей, доля не годится), QuoteMismatch (цена, MixedQuoter, число ног), узел.
pub async fn prepare(
    v: &Via<'_>,
    req: &TradeRequest,
    route: &DexRoute,
    mins: Option<&[Decimal]>,
    decs: (u8, u8),
    is_native: bool,
) -> GatewayResult<Prep> {
    let ctx = &v.q.ctx;
    let weights: Vec<Decimal> = route
        .legs
        .iter()
        .map(|l| {
            if req.side == Side::Buy {
                l.expected_out
            } else {
                l.share
            }
        })
        .collect();
    let total: Decimal = weights.iter().sum();
    if route.legs.is_empty() || total <= Decimal::ZERO {
        return Err(ctx.err(ErrorCode::NoRoute, "Умный маршрут без долей"));
    }
    let mut paths: Vec<(Path, Decimal)> = Vec::with_capacity(route.legs.len());
    let (mut coins, mut paid, mut left) = (Decimal::ZERO, Decimal::ZERO, req.qty);
    for (i, (leg, w)) in route.legs.iter().zip(&weights).enumerate() {
        let qty = if i + 1 == route.legs.len() {
            left
        } else {
            (req.qty * w / total).min(left)
        };
        left -= qty;
        let p = live::path(v, &leg.hops, qty, false).await?;
        paths.push((p, qty));
    }
    // Решение 223: квотер сверяет каждую долю отдельно на текущем состоянии сети — с ним сравнивается расчёт доли на том
    // же (исходном) состоянии; суммы, цена и шаги — по порядку исполнения на последовательных состояниях общих пулов
    // (quote/graph_seq.rs; без общих пулов — те же пути).
    let alone = paths.clone();
    graph_seq::resequence(ctx, req.side, v.taxes, &mut paths)?;
    for (p, _) in &paths {
        coins += p.coins;
        paid += p.usdt;
    }
    check_price(ctx, req, coins, paid)?;
    let legs: Vec<Leg<'_>> = alone.iter().map(|(p, _)| Leg { path: p }).collect();
    mixed_quoter::check(ctx, &legs).await?;
    let mut steps = Vec::with_capacity(paths.len());
    for (p, qty) in &paths {
        steps.push(chain::steps(ctx, req, p, *qty, decs)?);
    }
    // Графовый маршрут (решение 83): свежие минимумы выхода по ногам из верификации — последнему шагу каждой доли
    // (он отдаёт выход доли). Умный путь без графа передаёт None и ничего не меняет.
    if let Some(mins) = mins {
        if mins.len() != steps.len() {
            return Err(ctx.err(
                ErrorCode::QuoteMismatch,
                "верификация вернула число ног, не равное маршруту",
            ));
        }
        let out_dec = if req.side == Side::Buy {
            decs.0
        } else {
            decs.1
        };
        for (leg, min) in steps.iter_mut().zip(mins) {
            verify_gate::apply_leg_min(ctx, leg, *min, out_dec)?;
        }
    }
    let wrap = if is_native {
        native::adapt(ctx, req.side, &mut steps)?
    } else {
        Wrap::None
    };
    Ok(Prep { paths, steps, wrap })
}
