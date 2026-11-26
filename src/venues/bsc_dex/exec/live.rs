//! Файл: venues/bsc_dex/exec/live.rs
//! Что делает: путь доли маршрута из котировки, пересчитанный по живому состоянию его пулов прямо перед сделкой:
//! один пул (тот же расчёт, что у котировки, quote/direct.rs) или пара пулов через промежуточную монету
//! (quote/via.rs, решение 52). Допуск пула — по свежим итогам проверок (quote/select.rs); влияние крючка B/C — на сумме
//! сделки по факту пробы (решение 32; «335 (лесенка влияния)» — exec/hook_trade.rs: оценка → проба всех долей одним
//! пакетом → расчёт по лесенке сделки). Пара: промежуточная монета включена и без налогов,
//! у монеты с налогом — только V2 на каждом шаге (решение 39), прямой режим — оба пула одного Universal Router.
//! Доля графа из трёх и более пулов (решение 122 R2) — exec/live_hops.rs.
//! Кто вызывает: exec/trade_direct_prep.rs (path), exec/hook_trade.rs (share).
//! От чего зависит: exec/hook_trade.rs, exec/live_hops.rs, quote/direct.rs, quote/via.rs, quote/path.rs, quote/select.rs,
//! quote/direct_route.rs (один роутер), pools/registry.rs, pools/mids.rs, tokens.rs, exec/quoter_refetch.rs (решение 382:
//! перечитанные после расхождения с квотером пулы доли — Quoting.pinned — вместо памяти).
//! Настройки: через вызываемые — см. их шапки.

use rust_decimal::Decimal;

use crate::core::types::{RouteHop, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::quoter_amount;
use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::super::quote::direct::{self, Candidate};
use super::super::quote::path::Path;
use super::super::quote::via::Via;
use super::super::quote::{direct_route, pool_tax, select};
use super::hook_trade::{self, TradeHooks};

/// Живой путь одной доли с пробой крючков на её сумме (exec/hook_trade.rs paths). Принимает: расчёт путей, пулы доли из
/// котировки, количество монет доли, прямой ли режим (пара — одного Universal Router). Возвращает: путь и факты пробы
/// (ревью Н7: выход квотера на сумме сделки — сверке прямого режима). Ошибки: NoRoute (пул пропал, не допущен, пара не
/// годится, проба на сумме сделки не прошла), CoinNotChecked, QuoteMismatch, BadResponse, узел.
pub async fn path(
    v: &Via<'_>,
    hops: &[RouteHop],
    qty: Decimal,
    direct_mode: bool,
) -> GatewayResult<(Path, TradeHooks)> {
    let (mut p, th) = hook_trade::paths(v, &[(hops, qty)], direct_mode).await?;
    let one = p
        .pop()
        .ok_or_else(|| v.q.ctx.err(ErrorCode::NoRoute, "доля без пути"))?;
    Ok((one, th))
}

/// Путь доли по текущим проверкам и фактам пробы этой сделки (без узла, кроме decimals). Решение 122 R2: доля графа из
/// трёх и более пулов — exec/live_hops.rs. Принимает: как path, и факты пробы. Ошибки: как path, кроме пробы.
pub async fn share(
    v: &Via<'_>,
    hops: &[RouteHop],
    qty: Decimal,
    direct_mode: bool,
    th: &TradeHooks,
) -> GatewayResult<Path> {
    let (q, ctx) = (v.q, &v.q.ctx);
    if hops.len() > 2 {
        return super::live_hops::path(v, hops, qty, th).await;
    }
    let found = hops
        .iter()
        .map(|h| {
            // Решение 382: перечитанная после расхождения с квотером запись (Quoting.pinned), иначе из памяти.
            super::quoter_refetch::entry(q, &h.pool)
                .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "Пул маршрута больше не в реестре"))
        })
        .collect::<GatewayResult<Vec<Entry>>>()?;
    eval(v, &found, qty, direct_mode, th).await
}

/// Путь доли по текущим проверкам пулов (допуск, лесенка влияния крючка с фактами сделки — TradeHooks::apply).
/// Принимает: расчёт путей, пулы доли (записи из памяти), количество монет, прямой ли режим, факты пробы. Ошибки: как
/// share.
async fn eval(
    v: &Via<'_>,
    found: &[Entry],
    qty: Decimal,
    direct_mode: bool,
    th: &TradeHooks,
) -> GatewayResult<Path> {
    let (q, ctx) = (v.q, &v.q.ctx);
    let mut entries = Vec::with_capacity(found.len());
    for mut e in found.iter().cloned() {
        if let Some((wallet, rebate, adapter)) = super::oasis_direct::chosen(&e) {
            let at = super::quoter_refetch::block(q)
                .map(|b| format!("{b:#x}"))
                .unwrap_or_else(|| "latest".into());
            let head = super::super::quote::white_state::head(ctx, &at).await?;
            let m = Box::pin(super::super::quote::oasis_pricing::state(
                ctx, &e, wallet, head, adapter,
            ))
            .await?;
            if (adapter || rebate)
                && super::super::quote::oasis_pricing::use_rebate(&m, adapter) != rebate
            {
                return Err(ctx.err(
                    ErrorCode::StaleData,
                    "OasisSwap: изменилась ставка кошелька",
                ));
            }
            super::super::quote::oasis_pricing::apply(&mut e, &m, head, rebate);
        }
        if direct_mode
            && found.len() == 1
            && super::white_direct::selected()
            && e.family.variant == super::super::pools::family_variant::Variant::WhiteSwap
        {
            let at = super::quoter_refetch::block(q)
                .map(|b| format!("{b:#x}"))
                .unwrap_or_else(|| "latest".into());
            Box::pin(super::super::quote::white_state::refresh(ctx, &mut e, &at)).await?;
        }
        if direct_mode
            && found.len() == 1
            && e.family.variant == super::super::pools::family_variant::Variant::DeltaSwap
        {
            let at = super::quoter_refetch::block(q)
                .map(|b| format!("{b:#x}"))
                .unwrap_or_else(|| "latest".into());
            let head = super::super::quote::white_state::head(ctx, &at).await?;
            Box::pin(super::super::pools::deltaswap::refresh(ctx, &mut e, head)).await?;
        }
        if found.len() == 1
            && super::frax_direct::selected()
            && super::super::quote::frax_pricing::is_frax(&e)
        {
            let at = super::quoter_refetch::block(q)
                .map(|b| format!("{b:#x}"))
                .unwrap_or_else(|| "latest".into());
            let h = super::super::quote::white_state::head(ctx, &at).await?;
            Box::pin(super::super::quote::frax_pricing::refresh(ctx, &mut e, h)).await?;
        }
        let mut u = select::usable(q, &e, v.side, v.usdt_dec)
            .map_err(|why| ctx.err(ErrorCode::NoRoute, format!("{}: {why}", e.family.title)))?;
        th.apply(&e.pool.reference, v.side, quoter_amount::hooked(&e), &mut u);
        entries.push((e, u));
    }
    match entries.as_slice() {
        [(e, u)] => {
            // Решение «335 (лесенка влияния)»: допуск с влиянием крючка, применённым на входе этого расчёта.
            let (swapped, coins, usdt, u) =
                direct::calc_u(ctx, e, u, v.side, qty, (v.coin_dec, v.usdt_dec))?;
            let c = Candidate {
                entry: e.clone(),
                usable: u,
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
            // Решение 39 по налогу пула (решение 146): пул с налогом стороны > 0 — только V2.
            for (e, u) in [p1, p2] {
                pool_tax::v2_only(e, u)
                    .map_err(|w| ctx.err(ErrorCode::NoRoute, format!("{}: {w}", e.family.title)))?;
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
