//! Файл: venues/bsc_dex/exec/live.rs
//! Что делает: путь доли маршрута из котировки, пересчитанный по живому состоянию его пулов прямо перед сделкой:
//! один пул (тот же расчёт, что у котировки, quote/direct.rs) или пара пулов через промежуточную монету
//! (quote/via.rs, решение 52). Перед расчётом — повторная проба крючка B/C каждого пула (решение 32, exec/precheck.rs)
//! и допуск пула по свежим итогам проверок (quote/select.rs). Пара: промежуточная монета включена и без налогов,
//! у монеты с налогом — только V2 на каждом шаге (решение 39), прямой режим — оба пула одного Universal Router.
//! Кто вызывает: exec/trade.rs, exec/trade_smart.rs.
//! От чего зависит: exec/precheck.rs, quote/direct.rs, quote/via.rs, quote/path.rs, quote/select.rs,
//! quote/direct_route.rs (один роутер), pools/registry.rs, pools/mids.rs, tokens.rs.
//! Настройки: через вызываемые — см. их шапки.

use rust_decimal::Decimal;

use crate::core::types::{PoolKind, RouteHop, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::super::quote::direct::{self, Candidate};
use super::super::quote::path::Path;
use super::super::quote::via::Via;
use super::super::quote::{direct_route, select};
use super::precheck;

/// Живой путь доли. Принимает: расчёт путей (монета, сторона, налоги, decimals), пулы доли из котировки, количество
/// монет доли, прямой ли режим (пара — одного Universal Router). Ошибки: NoRoute (пул пропал, не допущен, пара не
/// годится), CoinNotChecked, QuoteMismatch, BadResponse, узел.
pub async fn path(
    v: &Via<'_>,
    hops: &[RouteHop],
    qty: Decimal,
    direct_mode: bool,
) -> GatewayResult<Path> {
    let (q, ctx) = (v.q, &v.q.ctx);
    let mut entries = Vec::with_capacity(hops.len());
    for h in hops {
        let e = q
            .reg
            .get(&h.pool)
            .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "Пул маршрута больше не в реестре"))?;
        precheck::hook_again(q, &e).await?;
        let u = select::usable(q, &e, v.side, v.usdt_dec)
            .map_err(|why| ctx.err(ErrorCode::NoRoute, format!("{}: {why}", e.family.title)))?;
        entries.push((e, u));
    }
    match entries.as_slice() {
        [(e, u)] => {
            let (swapped, coins, usdt) =
                direct::calc(ctx, e, u, v.side, qty, v.taxes, (v.coin_dec, v.usdt_dec))?;
            let c = Candidate {
                entry: e.clone(),
                usable: u.clone(),
                swapped,
                coins,
                usdt,
            };
            Path::one(ctx, c, v.side, (v.coin_dec, v.usdt_dec))
        }
        [a, b] => {
            // Покупка: a — «промежуточная — USDT», b — «монета — промежуточная»; продажа — наоборот.
            let (p1, p2) = if v.side == Side::Buy { (a, b) } else { (b, a) };
            let m = mid(v, &p1.0, &p2.0)?;
            v.mid_ok(&m).map_err(|w| ctx.err(ErrorCode::NoRoute, w))?;
            if v.taxed() && (p1.0.pool.kind != PoolKind::V2 || p2.0.pool.kind != PoolKind::V2) {
                return Err(ctx.err(
                    ErrorCode::NoRoute,
                    "у монеты налог — через промежуточную только V2",
                ));
            }
            if direct_mode {
                direct_route::one_router(ctx, &a.0, &b.0)
                    .map_err(|w| ctx.err(ErrorCode::NoRoute, w))?;
            }
            let dec = q.tokens.decimals(ctx, m.address).await?;
            v.calc(&m, dec, p1, p2, qty)
        }
        _ => Err(ctx.err(
            ErrorCode::NoRoute,
            "доля — один пул или два через промежуточную монету",
        )),
    }
}

/// Промежуточная монета пары: P1 — «m — USDT», P2 — «монета — m», m включена сейчас. Ошибки: NoRoute.
fn mid(v: &Via<'_>, p1: &Entry, p2: &Entry) -> GatewayResult<Mid> {
    let ctx = &v.q.ctx;
    let m = p1.coin;
    if p1.usdt != v.usdt || p2.usdt != m || p2.coin != v.coin {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "пулы доли не образуют путь через промежуточную монету",
        ));
    }
    v.q.mids
        .active(ctx)
        .into_iter()
        .find(|x| x.address == m)
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                format!("промежуточная монета {m} сейчас не включена"),
            )
        })
}
