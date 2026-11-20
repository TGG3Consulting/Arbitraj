//! Файл: core/ride_pair.rs
//! Что делает: решение 480 (ревью 2) — две котировки продажи для маржинального счёта (ride_quote::marginal: наша часть =
//! котировка на «сдвиг + объём» минус котировка на «сдвиг») на ОДНОМ состоянии площадки:
//! - биржа — один снимок стакана (контракт quote_snapshot / на пути ордера exec_book::book — тот же FrozenBook, по
//!   которому биржа котирует и живую котировку: gateway/frozen.rs), обе суммы — из него;
//! - DEX — снимка живой котировки у шлюза нет (quote_snapshot DEX — замороженный реестр без KyberSwap, в graph-режиме —
//!   нижняя оценка: venues/bsc_dex/frozen.rs), поэтому две живые котировки и проверка, что обе посчитаны по одному
//!   блоку пула (DexRoute.block); блоки разные или у котировки нет маршрута (агрегатор) — отказ, следующий тик
//!   повторит (одно состояние не доказано).
//!
//! Кто вызывает: core/ride_quote.rs (sell_after — расчёт), core/partial_back.rs («продать на A» — путь исполнения).
//! От чего зависит: dex_account.rs (venue_dir), venue_ticker.rs, trade_ops.rs (kind), exec_book.rs, costs_quote.rs
//! (classify), gateway (contract, frozen, exit_scope, patience), core/types.
//! Настройки: нет (базовое поведение).

use rust_decimal::Decimal;

use crate::core::types::{Asset, DexAccountId, Direction, Quote, Side, VenueId};
use crate::gateway::contract::{Venue, VenueKind};
use crate::gateway::errors::{GatewayError, GatewayResult};

use super::engine::Core;
use super::size::Fail;

/// Что делает: обе котировки DEX посчитаны по одному блоку пула. Принимает: площадку, котировку на «сдвиг + объём», на
/// «сдвиг». Ошибки: текст — блоки разные или маршрута нет.
pub fn same_state(v: &VenueId, total: &Quote, ahead: &Quote) -> Result<(), String> {
    match (&total.route, &ahead.route) {
        (Some(t), Some(h)) if t.block == h.block => Ok(()),
        (Some(t), Some(h)) => Err(format!(
            "{}: котировки на сдвиг + объём и на сдвиг — по разным блокам пула ({} и {}), одно состояние не доказано; \
             повторит следующий расчёт",
            v.0, t.block, h.block
        )),
        _ => Err(format!(
            "{}: у котировки нет маршрута и блока (агрегатор) — котировки на сдвиг + объём и на сдвиг по одному \
             состоянию пула не доказуемы",
            v.0
        )),
    }
}

/// Что делает: пара котировок продажи на площадке продажи направления для расчёта (живые, кошелёк направления).
/// Принимает: ядро, направление, наш объём, сдвиг. Возвращает: (на сдвиг + объём, на сдвиг) в монете ядра.
/// Ошибки: как у costs_quote::fetch (TooLarge — NoRoute, Stop — прочие); Stop — одно состояние DEX не доказано.
pub async fn live_pair(
    core: &Core,
    d: &Direction,
    qty: Decimal,
    off: Decimal,
) -> Result<(Quote, Quote), Fail> {
    let v = &d.sell_on;
    let x = super::dex_account::venue_dir(&core.inner.deps, d, v)
        .await
        .map_err(Fail::Stop)?;
    let on = super::venue_ticker::on_venue(core, &d.asset, v);
    let cls = |e: GatewayError| super::costs_quote::classify(&v.0, &e);
    let (t, h) = if super::trade_ops::kind(core, v).map_err(Fail::Stop)? == VenueKind::Cex {
        let s = x.quote_snapshot(&on, Side::Sell).await.map_err(cls)?;
        let t = s.quote(qty + off).await.map_err(cls)?;
        (t, s.quote(off).await.map_err(cls)?)
    } else {
        let t = x.quote(&on, Side::Sell, qty + off).await.map_err(cls)?;
        let h = x.quote(&on, Side::Sell, off).await.map_err(cls)?;
        same_state(v, &t, &h).map_err(Fail::Stop)?;
        (t, h)
    };
    let back = |q| super::venue_ticker::quote_back(core, &d.asset, v, q);
    Ok((back(t), back(h)))
}

/// Что делает: пара котировок продажи на пути исполнения (как trade_ops::sell_value: у биржи — стакан для ордера
/// exec_book, у DEX — котировка исполнения кошельком счёта). Принимает: ядро, счёт DEX (None — биржа), площадку,
/// монету, наш объём, сдвиг. Возвращает: (на сдвиг + объём, на сдвиг). Ошибки: текст — площадки нет, стакана или
/// котировки нет, одно состояние DEX не доказано.
pub async fn exec_pair(
    core: &Core,
    acc: Option<&DexAccountId>,
    v: &VenueId,
    a: &Asset,
    qty: Decimal,
    off: Decimal,
) -> Result<(Quote, Quote), String> {
    let x = core
        .inner
        .deps
        .gw
        .get_as(v, acc)
        .await
        .map_err(|e| e.message)?;
    let err = |e: GatewayError| format!("{}: {}", v.0, e.message);
    let (t, h) = if super::trade_ops::kind(core, v)? == VenueKind::Cex {
        let b = super::exec_book::book(core, v, a, Side::Sell)
            .await
            .map_err(|e| e.message)?;
        let t = b.snap.quote(qty + off).await.map_err(err)?;
        (t, b.snap.quote(off).await.map_err(err)?)
    } else {
        let on = super::venue_ticker::on_venue(core, a, v);
        let t = exec_quote(x.as_ref(), &on, qty + off).await.map_err(err)?;
        let h = exec_quote(x.as_ref(), &on, off).await.map_err(err)?;
        same_state(v, &t, &h)?;
        (t, h)
    };
    let back = |q| super::venue_ticker::quote_back(core, a, v, q);
    Ok((back(t), back(h)))
}

/// Котировка продажи DEX на пути исполнения (как trade_ops sell_quote_of: продажа выхода, exec_scope). Ошибки: шлюза.
async fn exec_quote(x: &dyn Venue, on: &Asset, n: Decimal) -> GatewayResult<Quote> {
    let exit = crate::gateway::exit_scope::scope(None, x.quote(on, Side::Sell, n));
    crate::gateway::patience::exec_scope(exit).await
}
