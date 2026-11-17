//! Файл: venues/bsc_dex/pools/depth_facts.rs
//! Что делает: пулы площадки для экрана «Монеты» (решение 73, Venue::pool_depths): глубина по направлениям в полосе
//! цены и вердикт (pools/depth_book.rs — посчитанное на текущем состоянии пула, иначе считается сейчас), спит ли пул,
//! достоверен ли, итог сверки с квотером (check/state.rs — по сторонам, cross_check_buy / cross_check_sell), tickSpacing, «не сверено (блок)» по сторонам и «память отстаёт от сети» (решение 261) — строками в details; пауза по сверке
//! на одном блоке — итог стороны «failed» с причиной «пауза: …» (цифры и блок в тексте).
//! Решение 339: отметка неудачи чтения пула (pool_fail.rs) — строкой «не прочитан: причина …» в details; пулы, ни
//! разу не прочитанные (pool_unread.rs, записи в реестре нет), — отдельными строками с полем unread (монета, пара,
//! вид), valid = false, причиной и следующей попыткой; ядро их не знает, api/coins.rs кладёт их в строку монеты.
//! Решение 401: у пула менеджера (V4 / Infinity) — строки пробы через исполнитель v3 кошельков
//! (exec/executor/pass_view.rs details).
//! Кто вызывает: venue.rs (pool_depths; pool_depths_of — только пулы маршрута, карточка сигнала решения 74).
//! От чего зависит: registry.rs, depth_book.rs, pool_fail.rs, pool_unread.rs, check/state.rs, tokens.rs, ctx.rs.
//! Настройки: через depth_book.rs.

use crate::core::types::{CrossCheck, PoolDepth, PoolRef, SideCheck, UnreadPool};
use crate::gateway::errors::GatewayResult;

use super::super::check::state::Checks;
use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::registry::Registry;

/// Пулы реестра (см. шапку): все или только перечисленные (only — пулы маршрута для карточки сигнала, решение 74).
/// Ошибки: USDT не известен (venue.quote_asset), узел (decimals USDT).
pub async fn all(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    checks: &Checks,
    only: Option<&[PoolRef]>,
) -> GatewayResult<Vec<PoolDepth>> {
    let usdt = tokens.usdt(ctx)?;
    let dec = tokens.decimals(ctx, usdt).await?;
    let mut out = Vec::new();
    for r in reg.refs() {
        if only.is_some_and(|o| !o.contains(&r)) {
            continue;
        }
        let Some(e) = reg.get(&r) else { continue };
        // Недостоверный пул — последнее посчитанное (состояние может быть старым); спящий — как есть.
        let got = if e.valid && !e.asleep {
            reg.depth().depth(ctx, reg, tokens, &e, dec)
        } else {
            reg.depth().last(&r).ok_or_else(|| {
                if e.asleep {
                    super::super::quote::select::ASLEEP.to_string()
                } else {
                    "данные пула недостоверны — глубина не считается".to_string()
                }
            })
        };
        let (depth, error) = match got {
            Ok(d) => (Some(d), None),
            Err(why) => (None, Some(why)),
        };
        let q = checks.pool(&r).quoter;
        // Решение 339: пул не прочитался — причина и когда следующее чтение (pool_fail.rs). Считается до переноса `r`.
        let fail_note = reg
            .fails()
            .get(&r)
            .map(|f| super::pool_fail::detail(&f, std::time::Instant::now()));
        // Решение 401: пул менеджера — проба через исполнитель v3 кошельков (прошёл / отвергнут / не проверено).
        let pass = super::super::exec::executor::pass_view::details(ctx, &e);
        out.push(PoolDepth {
            venue: ctx.venue(),
            pool: r,
            buy_usd: depth.as_ref().map(|d| d.band.buy.round_dp(2)),
            sell_usd: depth.as_ref().map(|d| d.band.sell.round_dp(2)),
            band_pct: depth.as_ref().map(|d| d.band_pct),
            min_usd: depth.as_ref().map(|d| d.min),
            full: depth.as_ref().is_some_and(|d| d.band.full),
            deep_buy: depth.as_ref().is_some_and(|d| d.deep_buy),
            deep_sell: depth.as_ref().is_some_and(|d| d.deep_sell),
            depth_error: error,
            asleep: e.asleep,
            valid: e.valid,
            cross_check_buy: side_check(&q.buy, q.buy_paused),
            cross_check_sell: side_check(&q.sell, q.sell_paused),
            // Решение 261: «не сверено (блок)» по сторонам — строками подробностей (итог стороны — прежний).
            details: e
                .pool
                .tick_spacing
                .map(|s| format!("tickSpacing {s}"))
                .into_iter()
                .chain(q.buy_note.as_ref().map(|n| format!("покупка: {n}")))
                .chain(q.sell_note.as_ref().map(|n| format!("продажа: {n}")))
                .chain(q.memory.clone())
                .chain(fail_note)
                .chain(pass)
                .collect(),
            unread: None,
        });
    }
    // Решение 339 (экран «Монеты»): пулы, ни разу не прочитанные (записи в реестре нет) — строкой с причиной.
    for (r, u) in reg.unread().list() {
        if only.is_some_and(|o| !o.contains(&r)) || reg.entry(&r).is_some() {
            continue;
        }
        out.push(unread_row(ctx, reg, tokens, usdt, r, &u));
    }
    Ok(out)
}

/// Строка экрана для пула, ни разу не прочитанного: недостоверен, глубины и сверки нет, причина и следующая попытка —
/// в details (отметка 339 — когда следующее чтение; без неё — прежний повтор на новых блоках). Принимает: окружение,
/// реестр, токены, адрес USDT, ссылку и отметку пула. Ошибок нет.
fn unread_row(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt: alloy_primitives::Address,
    r: PoolRef,
    u: &super::pool_unread::Unread,
) -> PoolDepth {
    let next = match reg.fails().get(&r) {
        Some(f) => super::pool_fail::detail(&f, std::time::Instant::now()),
        None => format!(
            "не прочитан: {} (повтор на новых блоках, не больше dex.pool_load_retries; затем — при переподключении \
             пачки или по событию пула)",
            u.why
        ),
    };
    let pair = if u.other == usdt {
        "USDT".to_string()
    } else {
        tokens
            .ticker_of(u.other)
            .unwrap_or_else(|| u.other.to_string())
    };
    let none = SideCheck {
        state: CrossCheck::NotChecked,
        reason: None,
    };
    PoolDepth {
        venue: ctx.venue(),
        pool: r,
        buy_usd: None,
        sell_usd: None,
        band_pct: None,
        min_usd: None,
        full: false,
        deep_buy: false,
        deep_sell: false,
        depth_error: Some(UNREAD.to_string()),
        asleep: false,
        valid: false,
        cross_check_buy: none.clone(),
        cross_check_sell: none,
        details: vec![next],
        unread: Some(UnreadPool {
            coin: u.coin,
            pair,
            family: u.family.id.to_string(),
            kind: u.family.kind,
            fee_pips: u.fee_pips,
        }),
    }
}

/// Причина «глубины нет» у пула, ни разу не прочитанного (экран «Монеты»).
pub const UNREAD: &str = "пул ещё ни разу не прочитан — в котировки и сделки не идёт";

/// Начало причины стороны на паузе — по нему экран «Глубина» показывает «ПАУЗА» (ui PoolDepthCell.svelte).
pub const PAUSE_PREFIX: &str = "пауза:";

/// Итог сверки стороны для экрана. Решение 261 (ревью): сторона на паузе, а последняя сверка дала иную причину или
/// сторона мелкая — показывается «пауза» (отметка держится до сверки, прошедшей на одном блоке) с последней причиной.
/// Принимает: итог стороны, отметку паузы. Возвращает: SideCheck ядра. Ошибок нет.
pub fn side_check(x: &super::super::check::quoter_sides::SideCheck, paused: bool) -> SideCheck {
    let held = |last: &str| {
        format!(
            "{PAUSE_PREFIX} держится до сверки, прошедшей на одном блоке; последняя сверка: {last}"
        )
    };
    match x {
        Some(Err(why)) if paused && !why.starts_with(PAUSE_PREFIX) => SideCheck {
            state: CrossCheck::Failed,
            reason: Some(held(why)),
        },
        None if paused => SideCheck {
            state: CrossCheck::Failed,
            reason: Some(held("сторона мелкая — не сверялась")),
        },
        None => SideCheck {
            state: CrossCheck::NotChecked,
            reason: None,
        },
        Some(Ok(())) => SideCheck {
            state: CrossCheck::Ok,
            reason: None,
        },
        Some(Err(why)) => SideCheck {
            state: CrossCheck::Failed,
            reason: Some(why.clone()),
        },
    }
}
