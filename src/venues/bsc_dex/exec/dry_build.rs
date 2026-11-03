//! Файл: venues/bsc_dex/exec/dry_build.rs
//! Что делает: сборка «сухой сделки DEX» (решение 121, C4) — тем же путём, что сделка, но без занятия очереди кошелька
//! (nonce) и без отправки: те же функции exec/* и quote/*, вызванные в том же порядке, что в exec/trade.rs,
//! exec/trade_smart.rs и exec/bundle_trade.rs. Графовый маршрут — предторговая верификация (quote/verify::run, как
//! verify_gate::gate, но без уведомления exec.verified / exec.verify_rejected — сухая сделка их не шлёт); форма —
//! quote/graph_route::graph_form. Прямой: живой путь (exec/live.rs, прямой режим), цена против худшей, шаги
//! (exec/chain.rs), сверка с квотером (exec/precheck.rs), план Universal Router своей DEX (exec/plan.rs, недостающие
//! разрешения — с узла). Умный / граф «один UR»: доли по живому состоянию, сверка с MixedQuoter, exec/universal.rs.
//! Бандл: доли, свежие минимумы ног из верификации, exec/bundle.rs (первый nonce 0 — в симуляции nonce не нужен).
//! Доли по живому состоянию и верификация графа — exec/dry_paths.rs.
//! Через исполнитель ArbExecutor (решение 122, часть X) — exec/executor/dry.rs: режим «через исполнитель» (любой маршрут,
//! код подставлен в симуляцию) или графовый «бандл» при заданном exec.executor_address (как сделка).
//! Кто вызывает: exec/dry_swap.rs.
//! Решение 122 F: в графовой форме хопы через пулы Maverick V2 сверяются с квотером (exec/precheck_quoter.rs).
//! От чего зависит: exec/{trade (общие шаги), live, chain, precheck, plan, universal, bundle, native, verify_gate}.rs,
//! quote/{verify, graph_route, mixed_quoter, direct, via, steps}.rs.
//! Настройки: через вызываемые — exec.dex_slippage_pct, dex.*, smart.*, verify.*.

use rust_decimal::Decimal;

use crate::core::types::{DexRoute, RouteMode, Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::quote::graph_route::{self, GraphForm};
use super::super::quote::mixed_quoter::{self, Leg};
use super::super::quote::via::Via;
use super::super::quote::{Quoting, direct};
use super::super::{check::simulate::Base, pools::registry::Entry};
use super::bundle::Bundle;
use super::chain::Step;
use super::dry_paths::{live_paths, verified};
use super::executor::dry::{DryMode, ExecTx};
use super::native::{self, Wrap};
use super::plan::Plan;
use super::trade::{check_price, sides, worst_positive};
use super::{bundle, chain, executor, plan, precheck, universal, verify_gate};

/// Что собрано: одна транзакция Universal Router (прямой / умный), бандл или вызов исполнителя ArbExecutor (решение 122).
pub enum Built {
    One(Plan),
    Many(Bundle),
    Exec(ExecTx),
}

/// Собранная сухая сделка.
pub struct Ready {
    pub built: Built,
    /// Шаги каждой доли (минимумы, входы).
    pub legs: Vec<Vec<Step>>,
    /// Монет и USDT по живому пересчёту (вся сделка).
    pub coins: Decimal,
    pub usdt: Decimal,
    pub form: String,
    /// Блок, по времени которого считан срок; поверх него — симуляция (через исполнитель — со временем головы + 1 с).
    pub at: Base,
}

/// Собирает сделку. Принимает: котировку, запрос (как для сделки), маршрут, кошелёк. Возвращает: Ready.
/// Ошибки: как у сделки до отправки — NoRoute, QuoteMismatch, StaleData, CoinNotChecked, NotConfigured, узел.
pub async fn build(
    q: &Quoting,
    req: &TradeRequest,
    route: &DexRoute,
    wallet: alloy_primitives::Address,
    mode: DryMode,
) -> GatewayResult<Ready> {
    let ctx = &q.ctx;
    worst_positive(ctx, req)?;
    let (mut shape, mut mins) = (route.mode, None);
    let mut form = match route.mode {
        RouteMode::Direct => "прямой".to_string(),
        RouteMode::Smart => "умный".to_string(),
        RouteMode::Graph => String::new(),
    };
    if route.mode == RouteMode::Graph {
        if !route.executable {
            let why = route
                .exec_note
                .clone()
                .unwrap_or_else(|| "граф-маршрут не исполним как есть (решение 84)".to_string());
            return Err(ctx.err(ErrorCode::NoRoute, why));
        }
        mins = Some(verified(q, req, route).await?);
        (shape, form) = match graph_route::graph_form(ctx, route) {
            GraphForm::Direct => (RouteMode::Direct, "граф: прямой".into()),
            GraphForm::OneUr => (RouteMode::Smart, "граф: один Universal Router".into()),
            GraphForm::Bundle => (RouteMode::Graph, "граф: бандл".into()),
        };
    }
    if shape == RouteMode::Direct && route.legs.len() != 1 {
        return Err(ctx.err(ErrorCode::NoRoute, "Прямой маршрут должен быть одной долей"));
    }
    let (coin, usdt, decs) = sides(q, req).await?;
    let taxes = direct::taxes(q, &req.asset)?;
    let v = Via {
        q,
        coin,
        usdt,
        side: req.side,
        taxes: &taxes,
        coin_dec: decs.0,
        usdt_dec: decs.1,
    };
    let paths = live_paths(&v, req, route, shape == RouteMode::Direct).await?;
    let (coins, paid) = paths
        .iter()
        .fold((Decimal::ZERO, Decimal::ZERO), |(c, u), (p, _)| {
            (c + p.coins, u + p.usdt)
        });
    check_price(ctx, req, coins, paid)?;
    if shape == RouteMode::Smart {
        let legs: Vec<Leg<'_>> = paths.iter().map(|(p, _)| Leg { path: p }).collect();
        mixed_quoter::check(ctx, &legs).await?;
    }
    let mut legs = Vec::with_capacity(paths.len());
    for (p, qty) in &paths {
        legs.push(chain::steps(ctx, req, p, *qty, decs)?);
    }
    if let Some(mins) = &mins {
        if mins.len() != legs.len() {
            let why = "верификация вернула число ног, не равное маршруту";
            return Err(ctx.err(ErrorCode::QuoteMismatch, why));
        }
        let out_dec = if req.side == Side::Buy {
            decs.0
        } else {
            decs.1
        };
        for (leg, min) in legs.iter_mut().zip(mins) {
            verify_gate::apply_leg_min(ctx, leg, *min, out_dec)?;
        }
    }
    let is_native = native::is_native(ctx, &req.asset);
    // Решение 122 (часть X): через исполнитель — режим «через исполнитель» (любой маршрут, код подставлен) или
    // графовый «бандл» при заданном exec.executor_address (как пойдёт сделка, exec/trade.rs).
    let deployed = executor::address(ctx)?;
    let takes = shape == RouteMode::Graph && executor::takes(ctx, route, is_native)?.is_some();
    let via_exec = mode == DryMode::Executor || takes;
    if via_exec {
        if is_native {
            let why = "нативная монета через исполнитель (решение 122) в v1 не исполняется";
            return Err(ctx.err(ErrorCode::NoRoute, why));
        }
        let (at, now) = q.heads.at(ctx).await?;
        let x = executor::dry::build(ctx, &legs, wallet, now, (mode, deployed)).await?;
        let form = format!("{form} → {}", x.form);
        let built = Built::Exec(x);
        return Ok(Ready {
            built,
            legs,
            coins,
            usdt: paid,
            form,
            at: Base::timed(at, now),
        });
    }
    if is_native && shape == RouteMode::Graph {
        let why = "нативная монета в мульти-роутерном бандле (решение 84) в v1 не исполняется";
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    let wrap = if is_native {
        native::adapt(ctx, req.side, &mut legs)?
    } else {
        Wrap::None
    };
    if shape != RouteMode::Direct {
        // Решение 122 F: хопы через пулы с портом чужой математики (Maverick V2) — сверка квотером, как в сделке.
        super::precheck_quoter::hops(ctx, &paths).await?;
    }
    if shape == RouteMode::Direct {
        for h in paths.iter().flat_map(|(p, _)| p.hops.iter()) {
            precheck::quoter_now(ctx, &h.entry, h.token_in, &h.swapped, h.usable.hook_pct).await?;
        }
    }
    let (at, now) = q.heads.at(ctx).await?;
    let built = match shape {
        RouteMode::Direct => Built::One(
            plan::build(ctx, &legs[0], wallet, now, false)
                .await?
                .with_native(wrap),
        ),
        RouteMode::Smart => Built::One(
            universal::build(ctx, &legs, wallet, now)
                .await?
                .with_native(wrap),
        ),
        RouteMode::Graph => {
            let out = if req.side == Side::Buy { coin } else { usdt };
            Built::Many(bundle::build(ctx, &legs, out, wallet, 0, now, false).await?)
        }
    };
    Ok(Ready {
        built,
        legs,
        coins,
        usdt: paid,
        form,
        // Решение 127: прямой, умный и графовый (бандл) — голова + 1 с, как исполнитель (раньше +12 с узла BSC).
        at: Base::timed(at, now),
    })
}

/// Пулы шагов всех долей по порядку. Ошибок нет.
pub fn entries(legs: &[Vec<Step>]) -> Vec<&Entry> {
    legs.iter().flatten().map(|s| &s.entry).collect()
}
