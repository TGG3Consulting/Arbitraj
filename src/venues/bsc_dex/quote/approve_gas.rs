//! Файл: venues/bsc_dex/quote/approve_gas.rs
//! Что делает: газ разрешений для котировки (решение 35: «газ approve, первый раз на монету»): есть ли у кошелька
//! разрешения ERC-20 → Permit2 и Permit2 → Universal Router маршрута на сумму сделки; нет — газ, измеренный пробным
//! обменом в первом пуле маршрута (разрешения одни на токен входа первого шага, от пула не зависят).
//! Маршрут через исполнитель ArbExecutor (решение 122, часть X): ERC-20 allowance кошелька исполнителю (память).
//! Решение 271 (ревью): графовый маршрут, первый хоп которого — форк, без исполнителя идёт бандлом, и нога форка
//! берёт ERC-20 approve РОУТЕРУ ФОРКА (dex.router, без Permit2 — exec/bundle.rs, wallet/approve.rs::fork_missing):
//! ERC-20 allowance кошелька роутеру форка (память); dex.universal_router у форка не нужен и не читается.
//! Решение 271 (дополнение): ИСПОЛНИМЫЙ граф формы «бандл» без исполнителя — газ разрешений по ВСЕМ группам бандла
//! (группы Universal Router и пары «токен — роутер форка», как exec/bundle.rs) — quote/approve_bundle.rs; ветка
//! «первый хоп — форк» ниже остаётся только у неисполнимого графа (сигнал).
//! Кто вызывает: quote/mod.rs.
//! От чего зависит: quote/approve_bundle.rs, quote/graph_exec.rs (graph_form), exec/executor/mod.rs (takes), wallet/approve.rs, wallet/allow_cache.rs, wallet/signer.rs, check/state.rs, pools/registry.rs, num.rs.
//! Настройки: dex.universal_router, dex.permit2 [вид пула] (прямой маршрут), dex.router [форк первого хопа], smart.universal_router, smart.permit2
//! [bsc_dex] (умный); через approve.rs — dex.approve_unlimited, dex.permit2_expiration_s.

use crate::core::types::{Quote, RouteMode, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::num;
use super::super::wallet::{approve, signer};
use super::Quoting;

/// Газ разрешений, если их у кошелька ещё нет (решение 35: «газ approve, первый раз на монету»): текущие разрешения
/// ERC-20 → Permit2 и Permit2 → Universal Router маршрута против суммы сделки; газ — измеренный пробным обменом
/// в первом пуле маршрута (разрешения одни на токен, от пула не зависят).
/// Возвращает: единицы газа или None — разрешения уже есть. Ошибки: ключ кошелька, адреса, узел, газ не измерен.
pub async fn units(
    q: &Quoting,
    quote: &Quote,
    coin_dec: u8,
    usdt_dec: u8,
) -> GatewayResult<Option<u64>> {
    Ok(details(q, quote, coin_dec, usdt_dec).await?.map(|(u, _)| u))
}

/// Суммарный замер газа и количество отдельных транзакций разрешений (плата за публикацию Base начисляется каждой).
pub async fn details(
    q: &Quoting,
    quote: &Quote,
    coin_dec: u8,
    usdt_dec: u8,
) -> GatewayResult<Option<(u64, usize)>> {
    let ctx = &q.ctx;
    // Продажа нативной BNB (решение 62): платит роутер из WRAP_ETH — разрешений кошелька нет (exec/native.rs).
    if quote.side == Side::Sell && super::super::exec::native::is_native(ctx, &quote.asset) {
        return Ok(None);
    }
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма не переводится в единицы токена",
        )
    };
    let route = quote
        .route
        .as_ref()
        .ok_or_else(|| ctx.err(ErrorCode::Internal, "у котировки нет маршрута"))?;
    let hop = route
        .legs
        .first()
        .and_then(|l| l.hops.first())
        .ok_or_else(|| ctx.err(ErrorCode::Internal, "в маршруте нет пула"))?;
    let e = q
        .reg
        .get(&hop.pool)
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "пул маршрута больше не в реестре"))?;
    let (token, amount) = match quote.side {
        Side::Buy => (
            e.usdt,
            num::to_units(quote.avg_price * quote.fillable_qty, usdt_dec),
        ),
        Side::Sell => (e.coin, num::to_units(quote.fillable_qty, coin_dec)),
    };
    let amount = amount.ok_or_else(bad)?;
    if matches!(&e.pool.state,crate::core::types::PoolState::Carbon(s) if s.native_side.and_then(|i|s.tokens.get(i)).copied()==Some(token)){return Ok(None);}
    if super::ekubo_v2_pricing::enabled(&e) && matches!(&e.pool.state,crate::core::types::PoolState::Ekubo(d) if d.native==Some(token)) {return Ok(None);}
    if quote.side == Side::Buy
        && super::vista_pricing::native_router(&e)
        && ctx
            .text("venue.quote_asset")
            .is_ok_and(|t| super::vista_pricing::cash(ctx, &t, token))
    {
        return Ok(None);
    }
    // Решение 122 (часть X): маршрут через исполнитель ArbExecutor — одно разрешение ERC-20 исполнителю, без Permit2
    // (память — wallet/allow_cache.rs::erc20). Газ — тот же измеренный пробным обменом (верхняя оценка).
    let native = super::super::exec::native::is_native(ctx, &quote.asset);
    if let Some(x) = super::super::exec::executor::takes(ctx, &q.reg, route, native)? {
        let wallet = signer::address(ctx)?;
        if q.allow.erc20(ctx, token, wallet, x).await? >= amount {
            return Ok(None);
        }
        return Ok(measured(q, &e.pool.reference, quote.side)?.map(|u| (u, 1)));
    }
    // Решение 271 (дополнение): исполнимый граф формы «бандл» без исполнителя (exec/trade.rs → exec/bundle_trade.rs) —
    // разрешения по всем группам бандла, а не только по первому хопу первой доли (quote/approve_bundle.rs).
    if route.mode == RouteMode::Graph
        && route.executable
        && super::graph_exec::graph_form(ctx, route) == super::graph_exec::GraphForm::Bundle
    {
        return super::approve_bundle::details(q, quote, route, (coin_dec, usdt_dec)).await;
    }
    // Решение 271 (ревью): первый хоп — форк (граф, без исполнителя) — нога форка в бандле: одно ERC-20 approve роутеру
    // форка на токен входа (exec/bundle.rs, wallet/approve.rs::fork_missing — тот же allowance(кошелёк, роутер)).
    // Решение 280: первый хоп — V2-форк без роутера (без исполнителя маршрут — сигнал): роутера нет, разрешение пошло
    // бы исполнителю — газ, измеренный пробой (dex.router не читается).
    if route.mode == RouteMode::Direct
        && (super::vista_pricing::native_router(&e)
            || super::oasis_pricing::is_oasis(&e)
            || super::frax_pricing::is_frax(&e)
            || super::white_pricing::is_white(&e)
            || super::router_pricing::is_native(&e)
            || super::super::exec::antfarm_wallet::enabled(&e) || super::super::exec::temple_wallet::enabled(&e) || super::super::exec::carbon_direct::enabled(&e) || super::super::exec::mint_club_direct::enabled(&e) || super::ekubo_v2_pricing::enabled(&e) || super::super::exec::vault_direct::enabled(&e)
            || e.family.variant.umbrae() || super::super::pools::sectorone::enabled(&e) || super::delta_pricing::is_delta(&e))
    {
        let router = if e.family.variant.umbrae() || super::super::exec::antfarm_wallet::enabled(&e) || super::super::exec::temple_wallet::enabled(&e){match hop.fee_mode{Some(crate::core::types::RouterFeeMode::ProtectedWallet{target})=>target,_=>return Err(bad())}}else if super::super::exec::carbon_direct::enabled(&e){match hop.fee_mode{Some(crate::core::types::RouterFeeMode::CarbonWallet{module:Some(m),..})=>m,_=>ctx.dex_addr("dex.factory",e.family.id)?}}else if super::super::exec::mint_club_direct::enabled(&e) {match hop.fee_mode {Some(crate::core::types::RouterFeeMode::ProtectedWallet{target})=>target,_=>ctx.dex_addr("dex.factory",e.family.id)?}} else if super::ekubo_v2_pricing::enabled(&e) {ctx.dex_addr("dex.quoter",e.family.id)?} else if super::super::exec::vault_direct::enabled(&e) {ctx.dex_addr("dex.vault",e.family.id)?} else if matches!(
            hop.fee_mode,
            Some(
                crate::core::types::RouterFeeMode::NativeGrossMinimum
                    | crate::core::types::RouterFeeMode::NativeSettlement {
                        gross_minimum: true
                    }
            )
        ) || (e.family.variant==super::super::pools::family_variant::Variant::BunnySwap && (native || matches!(hop.fee_mode,Some(crate::core::types::RouterFeeMode::NativeSettlement{..})))) || super::oasis_pricing::is_oasis(&e)
            || super::frax_pricing::is_frax(&e)
        {
            ctx.dex_addr("dex.quoter", e.family.id)?
        } else {
            super::super::pools::deltaswap::router(ctx, &e)?
        };
        let wallet = signer::address(ctx)?;
        let current = q.allow.erc20(ctx, token, wallet, router).await?;
        let count = approve::fork_known(ctx, token, router, current, amount)?.len();
        if count == 0 {
            return Ok(None);
        }
        return Ok(measured(q, &e.pool.reference, quote.side)?.map(|units| (units, count)));
    }
    if super::v2_pair_quote::on(&e) {
        return Ok(measured(q, &e.pool.reference, quote.side)?.map(|u| (u, 1)));
    }
    if route.mode == RouteMode::Graph && e.family.fork {
        let router = ctx.dex_addr("dex.router", e.family.id)?;
        let wallet = signer::address(ctx)?;
        if q.allow.erc20(ctx, token, wallet, router).await? >= amount {
            return Ok(None);
        }
        return Ok(measured(q, &e.pool.reference, quote.side)?.map(|u| (u, 1)));
    }
    let (ur, permit2) = if route.mode == RouteMode::Smart {
        (
            ctx.addr("smart.universal_router")?,
            ctx.addr("smart.permit2")?,
        )
    } else {
        (
            ctx.dex_addr("dex.universal_router", e.family.id)?,
            ctx.dex_addr("dex.permit2", e.family.id)?,
        )
    };
    let wallet = signer::address(ctx)?;
    let (_, now) = q.heads.at(ctx).await?;
    // Разрешения — из памяти до транзакции кошелька (wallet/allow_cache.rs, решение 61), не с узла на каждый тик.
    let r = q.allow.read(ctx, token, wallet, permit2, ur).await?;
    let approvals = approve::lacking(ctx, &r, (token, permit2, ur), amount, now)?;
    if approvals.is_empty() {
        return Ok(None);
    }
    Ok(measured(q, &e.pool.reference, quote.side)?.map(|u| (u, approvals.len())))
}

/// Газ разрешений, измеренный пробным обменом в пуле (сторона покупки или продажи). Ошибки: CoinNotChecked — не измерен.
/// Зовёт также quote/approve_bundle.rs.
pub(super) fn measured(
    q: &Quoting,
    pool: &crate::core::types::PoolRef,
    side: Side,
) -> GatewayResult<Option<u64>> {
    // Решение 409: факты пула без копии.
    let f = q.checks.pool_arc(pool);
    let g = if side == Side::Buy {
        f.gas_approve_buy
    } else {
        f.gas_approve_sell
    };
    g.map(Some).ok_or_else(|| {
        q.ctx.err(
            ErrorCode::CoinNotChecked,
            "газ разрешений не измерен пробным обменом",
        )
    })
}
