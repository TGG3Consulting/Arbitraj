//! Файл: core/ride_pair.rs
//! Что делает: решение 480 (ревью 2, 3) — две котировки продажи для маржинального счёта (ride_quote::marginal: наша
//! часть = котировка на «сдвиг + объём» минус котировка на «сдвиг»):
//! - биржа — один снимок стакана (контракт quote_snapshot в расчёте / exec_book::book на пути исполнения — тот же
//!   FrozenBook, по которому биржа котирует и живую котировку: gateway/frozen.rs), обе суммы — из него (from_snapshot);
//! - DEX — две живые котировки, как в решении 394 (ревью 3: проверка одного блока убрана — у KyberSwap и «лучшего из
//!   двух» маршрута нет, а DexRoute.block — наименьший блок пулов маршрута, маршруты на «сдвиг + объём» и «сдвиг»
//!   разные). Факт: состояние пулов двух котировок может различаться на событие между ними (миллисекунды); сдвиг —
//!   заведомо консервативный (наши продажи впереди считаются ещё не исполненными).
//!   Ревью 4: обе котировки DEX — из ОДНОГО источника, своей математикой адаптера по реестру пулов
//!   (gateway::quote_stage::own_scope). Факт кода (venues/bsc_dex/venue.rs quote): источник выбирает адаптер по
//!   признаку финальной котировки (quote_stage::is_final) и exec.via_kyber — KyberSwap, при NoRoute своя, при «лучшем
//!   из двух» (exec.dex_best_of) — лучшая из двух, каждый вызов отдельно; задать «только KyberSwap» нельзя (NoRoute —
//!   своя), «только своя» — признак own_scope (адаптер читает quote_stage::own_only; is_final не меняется — исполнимый
//!   маршрут графа, решение 300, и ярус счёта как у финальной). Поэтому пара не смешивает исполнителей, а запросов к
//!   KyberSwap у пары нет (лимит kyber.rps не тратится). Исполнение продажи — по-прежнему выбранным путём (trade);
//!   цена решения при сдвиге — своя математика, а не цена KyberSwap (решение 95 — только без сдвига).
//!
//! Кто вызывает: core/ride_quote.rs (sell_after — расчёт), core/partial_back.rs («продать на A» — путь исполнения).
//! От чего зависит: dex_account.rs (venue_dir), venue_ticker.rs, trade_ops.rs (kind), exec_book.rs, costs_quote.rs
//! (classify), gateway (contract, frozen, exit_scope, patience, quote_stage), core/types.
//! Настройки: нет (базовое поведение).

use rust_decimal::Decimal;

use crate::core::types::{Asset, DexAccountId, Direction, Quote, Side, VenueId};
use crate::gateway::contract::{Venue, VenueKind};
use crate::gateway::errors::{GatewayError, GatewayResult};
use crate::gateway::frozen::FrozenQuote;

use super::engine::Core;
use super::size::Fail;

/// Что делает: обе котировки пары из одного снимка (биржа). Принимает: снимок, наш объём, сдвиг. Возвращает: (на сдвиг +
/// объём, на сдвиг). Ошибки: как у котировки снимка.
pub async fn from_snapshot(
    s: &dyn FrozenQuote,
    qty: Decimal,
    off: Decimal,
) -> GatewayResult<(Quote, Quote)> {
    let t = s.quote(qty + off).await?;
    Ok((t, s.quote(off).await?))
}

/// Что делает: пара котировок продажи на площадке продажи направления для расчёта (живые, кошелёк направления).
/// Принимает: ядро, направление, наш объём, сдвиг. Возвращает: (на сдвиг + объём, на сдвиг) в монете ядра.
/// Ошибки: как у costs_quote::fetch (TooLarge — NoRoute, Stop — прочие).
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
        from_snapshot(s.as_ref(), qty, off).await.map_err(cls)?
    } else {
        // Ревью 4: обе — своей математикой адаптера (один источник, без запросов к KyberSwap).
        let t = own_quote(x.as_ref(), &on, qty + off).await.map_err(cls)?;
        (t, own_quote(x.as_ref(), &on, off).await.map_err(cls)?)
    };
    let back = |q| super::venue_ticker::quote_back(core, &d.asset, v, q);
    Ok((back(t), back(h)))
}

/// Что делает: пара котировок продажи на пути исполнения (как trade_ops::sell_value: у биржи — стакан для ордера
/// exec_book, у DEX — котировка исполнения кошельком счёта; `deal` — id сделки, чьи монеты продаются: адаптер берёт
/// налоги, перепроверенные кошельком сделки, t78 — gateway/exit_scope.rs; None — оценка не продажи сделки).
/// Принимает: ядро, счёт DEX (None — биржа), площадку, монету, наш объём, сдвиг, id сделки. Возвращает: (на сдвиг +
/// объём, на сдвиг). Ошибки: текст — площадки нет, стакана или котировки нет.
pub async fn exec_pair(
    core: &Core,
    acc: Option<&DexAccountId>,
    deal: Option<&str>,
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
        from_snapshot(b.snap.as_ref(), qty, off).await.map_err(err)?
    } else {
        let on = super::venue_ticker::on_venue(core, a, v);
        let t = exec_quote(x.as_ref(), &on, qty + off, deal).await.map_err(err)?;
        (t, exec_quote(x.as_ref(), &on, off, deal).await.map_err(err)?)
    };
    let back = |q| super::venue_ticker::quote_back(core, a, v, q);
    Ok((back(t), back(h)))
}

/// Котировка продажи DEX своей математикой адаптера (ревью 4: не финальная — один источник у пары). Ошибки: шлюза.
async fn own_quote(x: &dyn Venue, on: &Asset, n: Decimal) -> GatewayResult<Quote> {
    crate::gateway::quote_stage::own_scope(x.quote(on, Side::Sell, n)).await
}

/// Котировка продажи DEX на пути исполнения (как trade_ops sell_quote_of: продажа выхода сделки `deal`, exec_scope),
/// своей математикой адаптера (ревью 4, own_quote). Ошибки: шлюза.
async fn exec_quote(
    x: &dyn Venue,
    on: &Asset,
    n: Decimal,
    deal: Option<&str>,
) -> GatewayResult<Quote> {
    let exit = crate::gateway::exit_scope::scope(deal, own_quote(x, on, n));
    crate::gateway::patience::exec_scope(exit).await
}
