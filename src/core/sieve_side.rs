//! Файл: core/sieve_side.rs
//! Что делает: решение 372 — данные одной стороны для отсева по границе (sieve.rs): биржа — снимок стакана адаптера
//! (контракт quote_snapshot: копия стакана, без запросов) и котировка снимка на всю глубину (ставка комиссии тейкера);
//! DEX — строгий верх курса (контракт rate_bound). Плюс факт прошлого перевода для поправки вывода (withdraw_seen.rs).
//! Ревью t162, Н7: стакан не копируется второй раз — граница читает его прямо из снимка (снимок и есть копия адаптера).
//! Выделено из sieve.rs (ревью t162, Н10: файл ≤ 200 строк).
//! Кто вызывает: core/sieve.rs.
//! От чего зависит: bound_in.rs (SideIn, Seen), dex_account.rs, venue_ticker.rs, withdraw_seen.rs, gateway (контракт:
//! quote_snapshot, rate_bound; frozen.rs QuoteSnapshot).
//! Настройки: transfer.seen_cost [площадка A].

use rust_decimal::Decimal;

use crate::core::types::{Direction, Quote, Side, VenueId};
use crate::gateway::contract::VenueKind;
use crate::gateway::frozen::QuoteSnapshot;
use crate::gateway::rate_bound::{DexBound, RateBound};
use crate::settings::read::At;

use super::bound_in::{Seen, SideIn};
use super::dirctx::DirCtx;
use super::engine::{Core, rd};

/// Данные одной стороны: снимок стакана биржи (снимок, котировка снимка для ставки, тикер) или верх DEX.
pub(super) enum Got {
    Book(QuoteSnapshot, Box<Quote>, String),
    Dex(DexBound),
}

impl Got {
    /// Что делает: вход сборки. Возвращает: SideIn; None — у снимка нет стакана (проверено при получении). Ошибок нет.
    pub(super) fn as_in(&self) -> Option<SideIn<'_>> {
        match self {
            Got::Book(snap, sample, ticker) => Some(SideIn::Book {
                book: snap.book()?,
                sample: sample.as_ref(),
                ticker,
            }),
            Got::Dex(b) => Some(SideIn::Dex(b)),
        }
    }

    /// Что делает: коротко для причины: стакан (уровни) или верх DEX. Ошибок нет.
    pub(super) fn describe(&self) -> String {
        match self {
            Got::Book(snap, ..) => match snap.book() {
                Some(b) => format!(
                    "стакан {} уровней asks / {} bids",
                    b.asks.len(),
                    b.bids.len()
                ),
                None => "снимок без стакана".into(),
            },
            Got::Dex(x) => format!(
                "верх курса DEX {} ({} пулов, ≤ {} хопов)",
                x.rho.round_dp(10).normalize(),
                x.pools,
                x.hops
            ),
        }
    }
}

/// Что делает: сторона направления: DEX — rate_bound; биржа — снимок стакана и котировка снимка на всю глубину (ставка
/// комиссии). Принимает: ядро, направление, площадку, сторону. Возвращает: Got. Ошибки: (класс, причина) — метод не
/// действует.
pub(super) async fn side(
    core: &Core,
    d: &Direction,
    v: &VenueId,
    s: Side,
) -> Result<Got, (&'static str, String)> {
    let kind = rd(&core.inner.kinds)
        .iter()
        .find(|(id, _)| id == v)
        .map(|(_, k)| *k);
    let venue = super::dex_account::venue_dir(&core.inner.deps, d, v)
        .await
        .map_err(|e| ("no_data", e))?;
    let on = super::venue_ticker::on_venue(core, &d.asset, v);
    if kind == Some(VenueKind::Dex) {
        return match venue.rate_bound(&on, s) {
            RateBound::Bound(b) => Ok(Got::Dex(b)),
            RateBound::NotApplicable { class, why } => Err((class, format!("{}: {why}", v.0))),
        };
    }
    let snap = venue
        .quote_snapshot(&on, s)
        .await
        .map_err(|e| ("stale", format!("{}: {}", v.0, e.message)))?;
    let depth: Decimal = {
        let book = snap
            .book()
            .ok_or_else(|| ("no_data", format!("{}: снимок без стакана", v.0)))?;
        let levels = if s == Side::Buy {
            &book.asks
        } else {
            &book.bids
        };
        levels.iter().map(|l| l.qty).sum()
    };
    let sample = snap
        .quote(depth)
        .await
        .map_err(|e| ("no_data", format!("{}: {}", v.0, e.message)))?;
    Ok(Got::Book(snap, Box::new(sample), on.ticker))
}

/// Что делает: факт прошлого перевода для поправки вывода (withdraw_seen.rs adjust_at: transfer.seen_cost = да по
/// площадке A, факт есть, комиссия F сообщена). Принимает: ядро, контекст. Возвращает: Seen; иначе None. Ошибок нет.
pub(super) fn seen(core: &Core, cx: &DirCtx) -> Option<Seen> {
    cx.a.info.as_ref()?;
    let d = &cx.dir;
    let at = At {
        venue: Some(&d.buy_on.0),
        coin: None,
    };
    if !core
        .inner
        .deps
        .settings
        .flag("transfer.seen_cost", at)
        .unwrap_or(false)
    {
        return None;
    }
    let s = super::withdraw_seen::get_for(&d.asset, &d.buy_on, &d.sell_on)?;
    Some(Seen {
        fee: s.fee?,
        sent: s.sent,
    })
}
