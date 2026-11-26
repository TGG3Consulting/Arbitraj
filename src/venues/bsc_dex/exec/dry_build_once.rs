//! Файл: venues/bsc_dex/exec/dry_build_once.rs
//! Что делает: одна сборка сухой сделки DEX (решение 121, C4) — тело exec/dry_build.rs build до решения 382 (вынесено:
//! файл был больше 200 строк; логика та же, кроме сверок — exec/dry_checks.rs). Тем же путём, что сделка: графовый
//! маршрут — предторговая верификация и форма; доли по живому состоянию (exec/dry_paths.rs; по перечитанным пулам —
//! Quoting.pinned); прямая форма — сверка с квотером как в сделке (решения 177, 261, 382: продажа выхода по квотеру);
//! цена против худшей; умный — MixedQuoter; шаги и минимумы; исполнитель (решение 122, часть X) или план Universal
//! Router / бандл; хопы Maverick — квотером.
//! Кто вызывает: exec/dry_build.rs (build — с перечитыванием пулов при расхождении, решение 382).
//! От чего зависит: exec/{trade (общие шаги), dry_paths, dry_checks, chain, leg_min, plan, universal, bundle, native,
//! executor}.rs, quote/{graph_route, direct, via}.rs.
//! Настройки: через вызываемые — exec.dex_slippage_pct, dex.*, smart.*, verify.*.

use rust_decimal::Decimal;

use crate::core::types::{DexRoute, RouteMode, Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::simulate::Base;
use super::super::quote::graph_route::{self, GraphForm};
use super::super::quote::via::Via;
use super::super::quote::{Quoting, direct};
use super::dry_build::{Built, Ready};
use super::dry_paths::{live_paths, verified};
use super::executor::dry::DryMode;
use super::native::{self, Wrap};
use super::trade::{check_price, sides, worst_positive};
use super::{
    bundle, chain, dry_checks, executor, leg_min, plan, quoter_refetch, quoter_refetch_check,
    universal,
};

/// Одна сборка (тело build до решения 382; по перечитанным пулам — Quoting.pinned — квотер на их блоке).
/// Принимает и ошибки: как build.
pub(super) async fn once(
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
        RouteMode::Rfq(_)
        | RouteMode::RfqIndicative
        | RouteMode::Slot(_)
        | RouteMode::Orderbook(_)
        | RouteMode::OrderbookIndicative
        | RouteMode::SlotIndicative => {
            return Err(ctx.err(
                ErrorCode::NotApplicable,
                "Выбранная внешняя цена проверяется с кошельком в адаптере источника",
            ));
        }
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
    let mut paths = live_paths(&v, req, route, shape == RouteMode::Direct).await?;
    // Решения 177, 382: прямая форма — сверка с квотером как в сделке (exec/trade_direct_prep.rs): продажа выхода — по
    // квотеру (путь и минимум верификации — тем же множителем), до проверки цены, шагов и минимумов (exec/dry_checks.rs).
    let by_quoter = if shape == RouteMode::Direct && mode != DryMode::Executor {
        dry_checks::direct(q, req, &mut paths, &mut mins).await?
    } else {
        None
    };
    let (coins, paid) = paths
        .iter()
        .fold((Decimal::ZERO, Decimal::ZERO), |(c, u), (p, _)| {
            (c + p.coins, u + p.usdt)
        });
    check_price(ctx, req, coins, paid)?;
    // Решение 382, ревью Н1: по перечитанным пулам цена не хуже плана больше допуска сверки.
    if quoter_refetch::block(q).is_some() {
        quoter_refetch_check::plan_gap(ctx, req, coins, paid)?;
    }
    if shape == RouteMode::Smart {
        // Решение 261: односторонняя сверка; строки «в нашу сторону» сухой сделке не нужны (сделки нет).
        dry_checks::smart(q, &paths).await?;
    }
    let mut legs = Vec::with_capacity(paths.len());
    for (p, qty) in &paths {
        legs.push(chain::steps(ctx, req, p, *qty, decs)?);
    }
    // Решение 228: минимум доли = E_k × max(1 − допуск, B / E), свежий минимум верификации — нижней границей (как сделка).
    let shares = paths.iter().map(|(p, q)| (p, *q));
    leg_min::set_leg_mins(ctx, req, shares, &mut legs, mins.as_deref(), decs)?;
    let is_native = native::is_native(ctx, &req.asset);
    // Решение 122 (часть X): через исполнитель — режим «через исполнитель» (любой маршрут, код подставлен) или
    // графовый «бандл» при заданном exec.executor_address (как пойдёт сделка, exec/trade.rs).
    if legs.iter().flatten().any(|s|super::mint_club_direct::enabled(&s.entry)){
        let ([steps],[(path,_)])=(legs.as_slice(),paths.as_slice())else{return Err(ctx.err(ErrorCode::NoRoute,"Mint Club: требуется одна доля"));};
        let ([s],[h])=(steps.as_slice(),path.hops.as_slice())else{return Err(ctx.err(ErrorCode::NoRoute,"Mint Club: требуется один рынок"));};
        if mode!=DryMode::AsTrade || route.mode!=RouteMode::Direct || is_native || native::cash(ctx,&ctx.text("venue.quote_asset")?,usdt){return Err(ctx.err(ErrorCode::NoRoute,"Mint Club: требуется прямое поручение ERC20"));}
        let (height,time)=q.heads.at(ctx).await?;
        let p=Box::pin(super::mint_club_direct::path_plan(ctx,s,h,wallet,height)).await?;
        return Ok(Ready{built:Built::One(p),legs,coins,usdt:paid,form,by_quoter,at:Base::timed(height,time)});
    }
    let deployed = executor::address(ctx)?;
    let takes =
        shape == RouteMode::Graph && executor::takes(ctx, &q.reg, route, is_native)?.is_some();
    let via_exec = mode == DryMode::Executor || takes;
    if via_exec {
        // Решение 272: нативный BNB — executeNative: в режиме «через исполнитель» (код v2 подставлен) или у
        // развёрнутого исполнителя v2; иначе отказ. Задача t122 (п. 4): причина — native_block, как в сделке
        // (exec/executor/trade.rs, задача t114): не сверен / v1 — «переразверните исполнитель» / обёртка не та.
        if is_native
            && mode != DryMode::Executor
            && let Some(why) = executor::native_block(ctx)
        {
            // Ревью t122: тот же текст, что в боевой сделке (executor::native_refusal).
            return Err(ctx.err(ErrorCode::NoRoute, executor::native_refusal(&why)));
        }
        let nat = match native::wrapped(ctx) {
            Some((_, w)) if is_native => Some((w, req.side == Side::Sell)),
            _ => None,
        };
        let (at, now) = q.heads.at(ctx).await?;
        let x = executor::dry::build(ctx, &legs, wallet, now, (mode, deployed), nat).await?;
        let form = format!("{form} → {}", x.form);
        let built = Built::Exec(x);
        return Ok(Ready {
            built,
            legs,
            coins,
            usdt: paid,
            form,
            by_quoter,
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
        dry_checks::maverick(q, &paths).await?;
    }
    let (at, now) = q.heads.at(ctx).await?;
    let built = match shape {
        RouteMode::Rfq(_)
        | RouteMode::RfqIndicative
        | RouteMode::Slot(_)
        | RouteMode::Orderbook(_)
        | RouteMode::OrderbookIndicative
        | RouteMode::SlotIndicative => {
            return Err(ctx.err(ErrorCode::NotApplicable, "RFQ не является маршрутом пула"));
        }
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
        by_quoter,
        // Решение 127: прямой, умный и графовый (бандл) — голова + 1 с, как исполнитель (раньше +12 с узла BSC).
        at: Base::timed(at, now),
    })
}
