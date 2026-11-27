//! Файл: venues/bsc_dex/exec/trade_direct_prep.rs
//! Что делает: подготовка доли прямого маршрута по живому состоянию (вынесено из exec/trade_direct.rs — файл был больше
//! 200 строк; логика та же): путь (exec/live.rs), проверка худшей цены, шаги с минимумами (exec/chain.rs, решение 228 —
//! exec/leg_min.rs), нативная обёртка, сверка каждого хопа с квотером (решение 32): продажа выхода (решение 177) — по
//! квотеру (exec/exit_quote.rs); остальные — решение 261, односторонне (в нашу сторону — строка, exec/quoter_favour.rs).
//! Решение 382: расхождение расчёта по памяти с квотером против нас — пулы доли перечитываются на голове, доля
//! считается и сверяется заново на том же блоке (exec/quoter_refetch.rs).
//! Кто вызывает: exec/trade_direct.rs.
//! От чего зависит: exec/live.rs, exec/chain.rs, exec/leg_min.rs, exec/native.rs, exec/exit_quote.rs,
//! exec/quoter_favour.rs, exec/quoter_refetch.rs (решение 382), exec/trade.rs (check_price), quote/path.rs, quote/via.rs.
//! Настройки: через вызываемые — exec.dex_slippage_pct, dex.quoter_max_diff_pct, dex.*.

use rust_decimal::Decimal;

use crate::core::types::{RouteLeg, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::quote::path::{Hop, Path};
use super::super::quote::via::Via;
use super::chain::Step;
use super::native::{self, Wrap};
use super::trade::check_price;
use super::{
    chain, exit_quote, leg_min, live, quoter_favour, quoter_refetch, quoter_refetch_check,
};

/// Подготовленная доля: путь по живому состоянию, шаги с минимумами, нативная обёртка и пометка «продажа по квотеру»
/// (решение 177; None — по расчёту) и «в нашу сторону» (решение 261).
pub struct Prep {
    pub path: Path,
    pub steps: Vec<Step>,
    pub wrap: Wrap,
    /// Пометка исполнения: «продажа по квотеру» (177) и «в нашу сторону» (261) через «; ».
    pub note: Option<String>,
    /// Решение 261: только строка «в нашу сторону» — в журнал один раз перед отправкой (trade_direct.rs).
    pub favour: Option<String>,
}

/// Доля по живому состоянию: путь (exec/live.rs), проверка худшей цены, шаги с минимумами (exec/chain.rs), свежий
/// минимум графовой верификации последнему шагу, нативная обёртка, сверка каждого хопа с квотером (решение 32).
/// Решение 177: продажа выхода с расхождением — путь пересчитан по квотеру (exec/exit_quote.rs), затем снова проверка
/// худшей цены, шаги, минимумы (свежий минимум графа — тем же множителем) и обёртка; у покупки и без расхождения — как
/// раньше (тот же порядок, сверка — отказ).
/// Принимает: расчёт путей, запрос, долю, минимумы верификации, decimals, нативная ли монета. Ошибки: NoRoute,
/// QuoteMismatch, BadResponse, узел.
/// Решение 382: расчёт по памяти разошёлся с квотером (против нас) — пулы доли перечитываются на голове H
/// (exec/quoter_refetch.rs), доля считается заново по ним и сверяется с квотером на H; дальше — те же проверки.
pub async fn prepare(
    v: &Via<'_>,
    req: &TradeRequest,
    leg: &RouteLeg,
    mins: Option<&[Decimal]>,
    decs: (u8, u8),
    is_native: bool,
) -> GatewayResult<Prep> {
    let first = match Box::pin(once(v, req, leg, mins, decs, is_native)).await {
        Err(e) if quoter_refetch::wants(v.q, &e) => e,
        done => return done,
    };
    let pools: Vec<_> = leg.hops.iter().map(|h| h.pool.clone()).collect();
    let q = Box::pin(quoter_refetch::pinned(v.q, &pools, first)).await?;
    let v2 = quoter_refetch::via_on(v, &q);
    Box::pin(once(&v2, req, leg, mins, decs, is_native))
        .await
        .map_err(|e| quoter_refetch::on_block(&q, e))
}

/// Одна подготовка доли (тело prepare до решения 382; по перечитанным пулам — Quoting.pinned — квотер на их блоке).
/// Принимает и ошибки: как prepare.
async fn once(
    v: &Via<'_>,
    req: &TradeRequest,
    leg: &RouteLeg,
    mins: Option<&[Decimal]>,
    decs: (u8, u8),
    is_native: bool,
) -> GatewayResult<Prep> {
    let ctx = &v.q.ctx;
    // Тот же расчёт, что у котировки, но только по пулам маршрута и по состоянию на сейчас (с допуском пулов).
    if super::super::quote::fluid_hop_pricing::selected(&leg.hops).is_some(){
        if is_native{return Err(super::super::pools::fluid_dex_lite::bad(ctx));}
        let path=Box::pin(super::super::quote::fluid_hop_pricing::reprice(v,&leg.hops,req.qty)).await?;
        check_price(ctx,req,path.coins,path.usdt)?;
        let (steps,wrap)=legs(ctx,req,&path,mins,decs,false)?;
        return Ok(Prep{path,steps,wrap,note:None,favour:None});
    }
    let (mut path, th) = live::path(v, &leg.hops, req.qty, true).await?;
    check_price(ctx, req, path.coins, path.usdt)?;
    // Решение 382, ревью Н1: по перечитанным пулам цена не хуже плана больше допуска сверки.
    if quoter_refetch::block(v.q).is_some() {
        quoter_refetch_check::plan_gap(ctx, req, path.coins, path.usdt)?;
    }
    let (mut steps, mut wrap) = legs(ctx, req, &path, mins, decs, is_native)?;
    // Ревью Н7: у пула с крючком выход квотера на сумме хопа уже снят пробой этой сделки (exec/hook_trade.rs).
    let known = |h: &Hop| {
        th.quoted(
            &h.entry.pool.reference,
            h.token_in == h.entry.usdt,
            h.swapped.amount_in,
        )
    };
    // getAmountsOut MarSwap рассчитывает процентный режим. Платная нога проверяется
    // фактическим исполнением с подписанным msg.value, minOut и балансами кошелька.
    let native_flat=super::market_plan::Selection::for_leg(leg).native_flat.is_some();
    let funded_yield=super::yield_basis_requote::selected(&leg.hops).is_some();
    let pick = (exit_quote::exit_of(req), |h: &Hop| !funded_yield && !native_flat && !super::carbon_direct::enabled(&h.entry));
    // Решение 382: пулы перечитаны на блоке H — квотер на том же H (выход пробы крючка — с другого блока, не берётся).
    let all = match quoter_refetch::block(v.q) {
        Some(b) => super::quoter_refetch_check::check_on(ctx, &path, pick, b).await?,
        None => exit_quote::check_with(ctx, &path, pick, known).await?,
    };
    // Решение 261: расхождения «в нашу сторону» — только строка в хронологию (exec/quoter_favour.rs).
    let (fixes, favour) = quoter_favour::split(all);
    // Журнал — один раз перед отправкой (trade_direct.rs): после approve подготовка идёт второй раз.
    let favour = quoter_favour::line(&favour);
    if fixes.is_empty() {
        return Ok(Prep {
            path,
            steps,
            wrap,
            note: favour.clone(),
            favour,
        });
    }
    // Решение 177: ожидаемое — по квотеру; худшая цена и минимумы — заново от него.
    let r = exit_quote::rescale(ctx, &mut path, req.side, &fixes)?;
    check_price(ctx, req, path.coins, path.usdt)?;
    let mins = mins.map(|m| exit_quote::scale_mins(m, r));
    (steps, wrap) = legs(ctx, req, &path, mins.as_deref(), decs, is_native)?;
    let note = quoter_favour::join(exit_quote::report(ctx, req, &fixes).await, favour.clone());
    Ok(Prep {
        path,
        steps,
        wrap,
        note,
        favour,
    })
}

/// Шаги доли с минимумами (exec/chain.rs), свежий минимум графовой верификации последнему шагу, нативная обёртка.
/// Принимает: окружение, запрос, путь, минимумы верификации, decimals, нативная ли монета. Возвращает: шаги и обёртку.
/// Ошибки: NoRoute, QuoteMismatch, BadResponse, NotConfigured.
fn legs(
    ctx: &Ctx,
    req: &TradeRequest,
    path: &Path,
    mins: Option<&[Decimal]>,
    decs: (u8, u8),
    is_native: bool,
) -> GatewayResult<(Vec<Step>, Wrap)> {
    let mut legs = vec![chain::steps(ctx, req, path, req.qty, decs)?];
    // Решение 228 (exec/leg_min.rs): одна доля — наибольшее из границы сделки и ожидаемого × (1 − допуск), как раньше;
    // графовый маршрут (решение 83) — свежий минимум верификации нижней границей (выше ожидаемого — отказ).
    let ver =
        match mins {
            Some(mins) => Some(*mins.first().ok_or_else(|| {
                ctx.err(ErrorCode::NoRoute, "верификация не вернула минимум выхода")
            })?),
            None => None,
        };
    let ver = ver.as_ref().map(std::slice::from_ref);
    leg_min::set_leg_mins(ctx, req, [(path, req.qty)], &mut legs, ver, decs)?;
    let vista =
        path.hops.len() == 1 && (super::super::quote::vista_pricing::native_router(&path.hops[0].entry) || super::super::quote::ekubo_v2_pricing::enabled(&path.hops[0].entry));
    let wrap = if is_native && !vista {
        native::adapt(ctx, req.side, &mut legs)?
    } else {
        Wrap::None
    };
    Ok((legs.pop().unwrap_or_default(), wrap))
}
