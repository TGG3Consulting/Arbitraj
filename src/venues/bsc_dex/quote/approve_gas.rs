//! Файл: venues/bsc_dex/quote/approve_gas.rs
//! Что делает: газ разрешений для котировки (решение 35: «газ approve, первый раз на монету»): есть ли у кошелька
//! разрешения ERC-20 → Permit2 и Permit2 → Universal Router маршрута на сумму сделки; нет — газ, измеренный пробным
//! обменом в первом пуле маршрута (разрешения одни на токен входа первого шага, от пула не зависят).
//! Кто вызывает: quote/mod.rs.
//! От чего зависит: wallet/approve.rs, wallet/signer.rs, check/state.rs, pools/registry.rs, num.rs.
//! Настройки: dex.universal_router, dex.permit2 [вид пула] (прямой маршрут), smart.universal_router, smart.permit2
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
    let ctx = &q.ctx;
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
    let now = q.heads.fresh(ctx)?.timestamp;
    if approve::missing(ctx, token, wallet, permit2, ur, amount, now)
        .await?
        .is_empty()
    {
        return Ok(None);
    }
    let f = q.checks.pool(&e.pool.reference);
    let g = if quote.side == Side::Buy {
        f.gas_approve_buy
    } else {
        f.gas_approve_sell
    };
    g.map(Some).ok_or_else(|| {
        ctx.err(
            ErrorCode::CoinNotChecked,
            "газ разрешений не измерен пробным обменом",
        )
    })
}
