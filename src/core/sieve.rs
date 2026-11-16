//! Файл: core/sieve.rs
//! Что делает: решение 361 — отсев кандидата по ДОКАЗАННОЙ верхней границе прибыли перед тяжёлым расчётом (подбор
//! объёма с котировками DEX). Граница (bound.rs, сборка — bound_in.rs) строится только из памяти: стаканы бирж — снимок
//! адаптера (quote_snapshot: копия стакана, без запросов), сторона DEX — строгий верх курса адаптера (контракт
//! rate_bound, без котировок), вывод/возврат/запас — из контекста направления (dirctx.rs, те же функции, что расчёт).
//! Отсекает, только если доказано, что НИ ОДИН объём не пройдёт пороги: max U < money.min_profit_usd или (порог % ≥ 0 и
//! max(U − порог%·вложение) < 0). Метод не действует (кандидат идёт в расчёт как сейчас): внешний исполнитель
//! (exec.via_kyber), сторона DEX без строгого верха (вид пула, крючок, налоги, пулы между промежуточными при ≥ 3 хопах,
//! данные не свежие), нет снимка стакана, вход вне допустимого, любой сбой счёта.
//! Не в потоке тика: вызывается задачей расчёта кандидата (candidates.rs calc_one); замки рынка не держит (снимки —
//! копии адаптеров; контекст направления — те же короткие чтения, что у расчёта).
//! Кто вызывает: core/candidates.rs (calc_one).
//! От чего зависит: bound.rs, bound_in.rs, dirctx.rs, venue_dir.rs, profit.rs (final_external), dex_account.rs,
//! venue_ticker.rs, withdraw_seen.rs, gateway (контракт: quote_snapshot, rate_bound).
//! Настройки: money.min_profit_usd, money.min_profit_pct (как расчёт, venue_dir.rs), transfer.seen_cost [площадка A];
//! через dirctx.rs — profit.*, usdt_return.mode.

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{Direction, Side, VenueId};
use crate::gateway::contract::VenueKind;
use crate::gateway::rate_bound::RateBound;
use crate::settings::read::At;

use super::bound_in::{SideIn, assemble};
use super::dirctx::DirCtx;
use super::engine::{Core, rd};

/// Итог отсева.
#[derive(Debug, Clone)]
pub enum Sieve {
    /// Доказано: сделки на этих данных нет. Причина с числами и данные (граница, порог, из чего сложилась).
    Cut { reason: String, data: Value },
    /// Граница не исключает сделку — в расчёт.
    Pass,
    /// Метод не действует: класс (ключ счётчика) и причина — в расчёт как сейчас.
    Na { class: &'static str, why: String },
}

/// Что делает: отсев кандидата (см. шапку). Принимает: ядро, направление. Возвращает: Sieve. Ошибок нет.
pub async fn check(core: &Core, d: &Direction) -> Sieve {
    let na = |class: &'static str, why: String| Sieve::Na { class, why };
    if super::profit::final_external(core, d) {
        return na(
            "kyber",
            "финальная котировка — у внешнего исполнителя (exec.via_kyber)".into(),
        );
    }
    let cx = match DirCtx::build(core, d) {
        Ok(c) => c,
        Err(e) => return na("no_data", e),
    };
    let s = &core.inner.deps.settings;
    let (min_usd, min_pct) = match super::venue_dir::thresholds(s, super::venue_dir::of(core, d)) {
        Ok(t) => t,
        Err(e) => return na("no_data", e),
    };
    let a = match side(core, d, &d.buy_on, Side::Buy).await {
        Ok(x) => x,
        Err((c, w)) => return na(c, w),
    };
    let b = match side(core, d, &d.sell_on, Side::Sell).await {
        Ok(x) => x,
        Err((c, w)) => return na(c, w),
    };
    let seen = seen(core, &cx);
    let bound = match assemble(&cx, &a.as_in(), &b.as_in(), seen.as_ref()) {
        Ok(x) => x,
        Err(e) => return na("inputs", e),
    };
    let Some(best) = bound.best(Some(min_pct)) else {
        return na(
            "overflow",
            "граница не вычисляется (переполнение или приход не обратить)".into(),
        );
    };
    let r = |v: Decimal| v.round_dp(6).normalize();
    let mut data = json!({
        "best": best, "min_usd": min_usd, "min_pct": min_pct, "kappa": bound.kappa, "fixed": bound.fixed,
        "arrive": { "pct": bound.arrive.pct, "fix": bound.arrive.fix, "keep": bound.arrive.keep },
        "a": a.describe(), "b": b.describe(),
    });
    let how = format!(
        "κ = {}, фикс вывода {}, постоянные {}; A: {}; B: {}; изломов {}, объём до {}",
        r(bound.kappa),
        r(bound.arrive.fix),
        r(bound.fixed),
        a.describe(),
        b.describe(),
        best.points,
        r(best.q_hi)
    );
    if best.below_usd(min_usd) {
        let reason = format!(
            "отсев по границе: прибыль не больше {} USDT (на {} {}) < порога {min_usd} ({how})",
            r(best.max_u),
            r(best.q_u),
            d.asset.ticker
        );
        data["by"] = json!("usd");
        return Sieve::Cut { reason, data };
    }
    if best.below_pct()
        && let Some(v) = best.max_v
    {
        let reason = format!(
            "отсев по границе: прибыль ниже порога {min_pct}% на любом объёме (запас до порога {} USDT < 0; {how})",
            r(v)
        );
        data["by"] = json!("pct");
        return Sieve::Cut { reason, data };
    }
    Sieve::Pass
}

/// Данные одной стороны: снимок стакана биржи (стакан + котировка для ставки) или верх DEX.
enum Got {
    Book(
        Box<crate::core::types::OrderBook>,
        Box<crate::core::types::Quote>,
        String,
    ),
    Dex(crate::gateway::rate_bound::DexBound),
}

impl Got {
    /// Вход сборки. Ошибок нет.
    fn as_in(&self) -> SideIn<'_> {
        match self {
            Got::Book(book, sample, ticker) => SideIn::Book {
                book: book.as_ref(),
                sample: sample.as_ref(),
                ticker,
            },
            Got::Dex(b) => SideIn::Dex(b),
        }
    }

    /// Коротко для причины: стакан (уровни) или верх DEX. Ошибок нет.
    fn describe(&self) -> String {
        match self {
            Got::Book(b, ..) => format!(
                "стакан {} уровней asks / {} bids",
                b.asks.len(),
                b.bids.len()
            ),
            Got::Dex(x) => format!(
                "верх курса DEX {} ({} пулов, ≤ {} хопов)",
                x.rho.round_dp(10).normalize(),
                x.pools,
                x.hops
            ),
        }
    }
}

/// Сторона направления: DEX — rate_bound; биржа — снимок стакана и котировка снимка на всю глубину (ставка комиссии).
/// Ошибки: (класс, причина) — метод не действует.
async fn side(
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
    let book = snap
        .book()
        .cloned()
        .ok_or_else(|| ("no_data", format!("{}: снимок без стакана", v.0)))?;
    let levels = if s == Side::Buy {
        &book.asks
    } else {
        &book.bids
    };
    let depth: Decimal = levels.iter().map(|l| l.qty).sum();
    let sample = snap
        .quote(depth)
        .await
        .map_err(|e| ("no_data", format!("{}: {}", v.0, e.message)))?;
    Ok(Got::Book(Box::new(book), Box::new(sample), on.ticker))
}

/// Факт прошлого перевода для поправки вывода (withdraw_seen.rs adjust_at: transfer.seen_cost = да по площадке A,
/// факт есть, комиссия F сообщена). Ошибок нет: иначе None.
fn seen(core: &Core, cx: &DirCtx) -> Option<super::bound_in::Seen> {
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
    let s = super::withdraw_seen::get(&d.asset, &d.buy_on)?;
    Some(super::bound_in::Seen {
        fee: s.fee?,
        sent: s.sent,
    })
}
