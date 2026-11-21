//! Файл: venues/bsc_dex/quote/approve_bundle.rs
//! Что делает: газ разрешений котировки для графового маршрута, который исполняется БАНДЛОМ свопов без исполнителя
//! (решение 271, дополнение — план = факт). Разрешений столько же и по тем же правилам, что соберёт exec/bundle.rs::build:
//! - ноги, первый хоп которых НЕ форк, группируются по адресу Universal Router (dex.universal_router семейства) — на
//!   группу разрешения ERC-20 → Permit2 и Permit2 → UR (dex.permit2) на сумму входов её ног (wallet/approve.rs::missing);
//! - ноги форка (первый хоп — форк) группируются по паре (токен входа, роутер форка dex.router) — на пару один ERC-20
//!   approve роутеру на сумму входов её ног (wallet/approve.rs::fork_missing; одна пара — один approve, exec/bundle.rs).
//!
//! Вход ноги — доля входа маршрута (RouteLeg.share, quote/costs.rs) × вход сделки. Токен входа — как quote/approve_gas.rs:
//! покупка — встречный токен (usdt) записи первого пула ноги, продажа — монета (coin) записи.
//! Газ одной группы/пары, у которой разрешений не хватает, — замер пробы первого пула её первой ноги
//! (check/probe.rs gas_approve_* — все разрешения своего пути пула; верхняя оценка, как quote/approve_gas.rs::measured).
//! Разрешения кошелька — из памяти (wallet/allow_cache.rs: read — Permit2-пара, erc20 — роутер форка).
//! Кто вызывает: quote/approve_gas.rs (units); тесты (plan, txs).
//! От чего зависит: quote/approve_gas.rs (measured), wallet/{approve, allow_cache, signer}.rs, pools/registry.rs, num.rs.
//! Настройки: dex.universal_router, dex.permit2, dex.router [DEX]; через approve.rs — dex.approve_unlimited,
//! dex.permit2_expiration_s [bsc_dex].

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{DexRoute, PoolRef, Quote, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::num;
use super::super::wallet::{approve, signer};
use super::Quoting;
use super::approve_gas::measured;

/// Одна группа разрешений бандла: группа Universal Router или пара (токен, роутер форка).
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    /// true — нога форка (один ERC-20 approve роутеру форка); false — группа UR (ERC-20 → Permit2, Permit2 → UR).
    pub fork: bool,
    /// Роутер: роутер форка (dex.router) или Universal Router (dex.universal_router).
    pub router: Address,
    /// Permit2 группы UR (dex.permit2 её первой ноги); у форка — None.
    pub permit2: Option<Address>,
    /// Токен входа (первой ноги группы).
    pub token: Address,
    /// Сумма долей входа ног группы.
    pub share: Decimal,
    /// Первый пул первой ноги группы — по нему замер газа разрешений пробой.
    pub pool: PoolRef,
}

/// Группы разрешений маршрута — в порядке первого появления, как exec/bundle.rs::build. Принимает: котировку
/// (реестр, настройки), маршрут, сторону. Ошибки: Internal (доля без пула), NoRoute (пула нет в реестре), NotConfigured.
pub fn plan(q: &Quoting, route: &DexRoute, side: Side) -> GatewayResult<Vec<Unit>> {
    let ctx = &q.ctx;
    let mut out: Vec<Unit> = Vec::new();
    for leg in &route.legs {
        let hop = leg
            .hops
            .first()
            .ok_or_else(|| ctx.err(ErrorCode::Internal, "в доле маршрута нет пула"))?;
        let e = q
            .reg
            .get(&hop.pool)
            .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "пул маршрута больше не в реестре"))?;
        let token = if side == Side::Buy { e.usdt } else { e.coin };
        let fork = e.family.fork;
        // Форк: ключ — (токен, роутер форка); UR: ключ — адрес роутера (как группы exec/bundle.rs).
        let (router, permit2) = if fork {
            (ctx.dex_addr("dex.router", e.family.id)?, None)
        } else {
            let ur = ctx.dex_addr("dex.universal_router", e.family.id)?;
            (ur, Some(ctx.dex_addr("dex.permit2", e.family.id)?))
        };
        let same = |u: &Unit| u.fork == fork && u.router == router && (!fork || u.token == token);
        match out.iter_mut().find(|u| same(u)) {
            Some(u) => u.share += leg.share,
            None => out.push(Unit {
                fork,
                router,
                permit2,
                token,
                share: leg.share,
                pool: e.pool.reference.clone(),
            }),
        }
    }
    Ok(out)
}

/// Сколько транзакций разрешений соберёт бандл, если у кошелька нет ни одного разрешения (exec/bundle.rs::build с
/// all_approvals): у форка — одна (ERC-20 approve роутеру), у группы UR — две (ERC-20 → Permit2, Permit2 → UR). Ошибок нет.
pub fn txs(units: &[Unit]) -> usize {
    units.iter().map(|u| if u.fork { 1 } else { 2 }).sum()
}

/// Газ разрешений бандла: сумма замеров по группам, у которых разрешений не хватает. Принимает: котировку, маршрут,
/// decimals (монета, USDT). Возвращает: единицы газа или None — все разрешения есть. Ошибки: как у plan, ключ
/// кошелька, узел, CoinNotChecked (газ не измерен), BadResponse (сумма не переводится в единицы токена).
pub async fn units(
    q: &Quoting,
    quote: &Quote,
    route: &DexRoute,
    (coin_dec, usdt_dec): (u8, u8),
) -> GatewayResult<Option<u64>> {
    let ctx = &q.ctx;
    let (total, dec) = match quote.side {
        Side::Buy => (quote.avg_price * quote.fillable_qty, usdt_dec),
        Side::Sell => (quote.fillable_qty, coin_dec),
    };
    let wallet = signer::address(ctx)?;
    let mut sum: Option<u64> = None;
    for u in plan(q, route, quote.side)? {
        let amount = num::to_units(total * u.share, dec).ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "сумма не переводится в единицы токена",
            )
        })?;
        let lacking = match u.permit2 {
            None => q.allow.erc20(ctx, u.token, wallet, u.router).await? < amount,
            Some(permit2) => {
                let (_, now) = q.heads.at(ctx).await?;
                let r = q
                    .allow
                    .read(ctx, u.token, wallet, permit2, u.router)
                    .await?;
                !approve::lacking(ctx, &r, (u.token, permit2, u.router), amount, now)?.is_empty()
            }
        };
        if lacking {
            let g = measured(q, &u.pool, quote.side)?.unwrap_or_default();
            sum = Some(sum.unwrap_or_default().saturating_add(g));
        }
    }
    Ok(sum)
}
