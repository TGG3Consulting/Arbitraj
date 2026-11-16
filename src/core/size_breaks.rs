//! Файл: core/size_breaks.rs
//! Что делает: решение 344 (ревью t159 Б4, Н4, Н5) — изломы кривых пулов DEX в точки подбора объёма (решение 66).
//! Изломы стаканов бирж подбор берёт сам (size_points.rs); у DEX-стороны кусок кривой меняется на тике, бине, точке
//! лесенки — пик прибыли между двумя точками сетки подбор мог не увидеть. Здесь, при size.pool_breaks_max = L:
//! 1) только ПОСЛЕ обычного поиска (границы, изломы стаканов, сетка, уточнение пиков) — изломы не вытесняют его из
//!    предела size.max_evals (ревью t159 Б4) и не считаются до быстрого пересчёта предпроверки 266 (Н5: подбор
//!    вызывается только когда быстрый путь не принял);
//! 2) только пулы МАРШРУТА лучшей посчитанной точки (её котировки DEX-сторон: доли сплита, налог монеты, крючок, у
//!    бинов при продаже — комиссия на вход учтены адаптером, Н4) — контракт quote_breaks через Curve::breaks;
//! 3) только в [наименьший, наибольший] объём бисекции, не больше L изломов на кандидата — ближайшие к лучшему объёму
//!    (у вогнутой модели пик один и он там; дальние изломы — после него по приоритету), по две точки (шаг до и после,
//!    как у уровней стакана);
//! 4) локальные максимумы на точках изломов уточняются тернарным поиском (size_par.rs refine_all) — как пики сетки.
//!
//! Точек: ≤ 2·L + (пиков на изломах)·(2·size.refine_steps + 3), в пределе size.max_evals и мягкого срока.
//! Изломы — только точки подбора, не данные решения: сбой адаптера — без них.
//! Кто вызывает: core/size_search.rs (run), core/size_curve.rs (core_breaks — CoreCurve::breaks).
//! От чего зависит: core/size_memo.rs, core/size_par.rs, core/size.rs, core/dex_account.rs, gateway (quote_breaks).
//! Настройки: size.pool_breaks_max (через Params.ext.breaks_max), size.refine_steps (через Params).

use rust_decimal::Decimal;

use crate::core::types::Side;

use super::costs::Calc;
use super::dirctx::DirCtx;
use super::engine::Core;
use super::size::{Curve, Fail, Params};
use super::size_memo::Memo;
use super::size_par::refine_all;

/// Что делает: п. 1–4 шапки. Принимает: расчёты, параметры, границы бисекции (шаги), предел изломов L, ширину
/// остановки. Возвращает: сколько пиков уточнено. Ошибки: Stop.
pub async fn run<C: Curve>(
    m: &mut Memo<'_, C>,
    p: &Params,
    (lo, hi): (u64, u64),
    limit: u32,
    tol: u64,
) -> Result<u32, Fail> {
    let Some(best) = m.best() else {
        return Ok(0);
    };
    let Some(nb) = m.units(best.qty) else {
        return Ok(0);
    };
    let max = m.qty(hi);
    let (buy, sell) = m.curve.breaks(&best.data, max, limit).await;
    let mut cand: Vec<u64> = buy.iter().filter_map(|q| m.units(*q)).collect();
    // Изломы продажи — в объём покупки по приходу (аффинен по объёму — как уровни bids, size_points.rs).
    if !sell.is_empty()
        && let (Ok(a), Ok(b)) = (m.at(lo).await, m.at(hi).await)
        && b.arrive > a.arrive
    {
        for s in &sell {
            let q = a.qty + (*s - a.arrive) * (b.qty - a.qty) / (b.arrive - a.arrive);
            if q > Decimal::ZERO
                && let Some(n) = m.units(q)
            {
                cand.push(n);
            }
        }
    }
    cand.retain(|n| (lo..=hi).contains(n));
    cand.sort_unstable();
    cand.dedup();
    cand.sort_by_key(|n| n.abs_diff(nb));
    cand.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let mut pts: Vec<u64> = cand
        .iter()
        .flat_map(|n| [*n, n.saturating_add(1)])
        .filter(|k| *k <= hi)
        .collect();
    pts.sort_unstable();
    pts.dedup();
    if pts.is_empty() {
        return Ok(0);
    }
    if m.par() > 1 {
        m.at_many(&pts).await;
    }
    for n in &pts {
        m.profit(*n).await?;
    }
    // Пики на точках изломов среди всех допустимых посчитанных на [lo, hi].
    let ok: Vec<(u64, Decimal)> = m
        .map
        .range(lo..=hi)
        .filter_map(|(n, r)| r.as_ref().ok().map(|q| (*n, q.profit)))
        .collect();
    let mut spans = Vec::new();
    for i in 0..ok.len() {
        let left_ok = i == 0 || ok[i].1 >= ok[i - 1].1;
        let right_ok = i + 1 == ok.len() || ok[i].1 >= ok[i + 1].1;
        if !(left_ok && right_ok) || pts.binary_search(&ok[i].0).is_err() {
            continue;
        }
        let a = if i == 0 { ok[i].0 } else { ok[i - 1].0 };
        let b = if i + 1 == ok.len() {
            ok[i].0
        } else {
            ok[i + 1].0
        };
        spans.push((a, b));
    }
    if !spans.is_empty() && !m.capped() {
        refine_all(m, &spans, p.refine_steps, tol).await?;
    }
    Ok(u32::try_from(spans.len()).unwrap_or(u32::MAX))
}

/// Что делает: изломы DEX-сторон направления по маршрутам расчёта `c` (CoreCurve::breaks): сторона без маршрута —
/// биржа (изломы — её стакан, их ядро берёт само). Принимает: ядро, контекст направления, расчёт точки, наибольший
/// объём, предел. Возвращает: (объёмы покупки, объёмы продажи). Ошибок нет: сбой стороны — пусто.
pub async fn core_breaks(
    core: &Core,
    cx: &DirCtx,
    c: &Calc,
    max_qty: Decimal,
    limit: u32,
) -> (Vec<Decimal>, Vec<Decimal>) {
    let d = &cx.dir;
    let deps = &core.inner.deps;
    let mut out = (Vec::new(), Vec::new());
    let sides = [
        (Side::Buy, &d.buy_on, &c.buy_quote, cx.on_a()),
        (Side::Sell, &d.sell_on, &c.sell_quote, cx.on_b()),
    ];
    for (side, venue, quote, on) in sides {
        let Some(route) = quote.route.as_ref() else {
            continue;
        };
        // Решение 132: DEX — кошелёк строки маппинга направления.
        let Ok(v) = super::dex_account::venue_dir(deps, d, venue).await else {
            continue;
        };
        let Ok(b) = v.quote_breaks(&on, side, route, max_qty, limit).await else {
            continue;
        };
        if side == Side::Buy {
            out.0 = b;
        } else {
            out.1 = b;
        }
    }
    out
}
