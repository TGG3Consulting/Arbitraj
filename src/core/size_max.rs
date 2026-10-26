//! Файл: core/size_max.rs
//! Что делает: «максимальный объём» и «максимальная прибыль» карточки сигнала (решение 74) — БЕЗ ограничения бюджетом:
//! 1) оптимум прибыли — тем же поиском, что и сделка (size_search.rs, решение 66), но вместо бюджета — верхний предел
//!    поиска card.max_search_usdt (списание по худшей цене не больше него);
//! 2) наибольший НАЙДЕННЫЙ объём, при котором прибыль ещё проходит пороги money.min_profit_usd и money.min_profit_pct, —
//!    бисекцией по целым шагам между заведомо проходящим объёмом и наибольшим возможным (size.bisect_steps шагов), на тех
//!    же расчётах, что и оптимум (Memo). Бисекция считает, что проходящие объёмы идут подряд от оптимума вверх; при
//!    нескольких участках прибыльности дальний может быть пропущен — так и написано в карточке;
//! 3) чем ограничен поиск сверху: пределом card.max_search_usdt или площадками — глубиной загруженных уровней стакана,
//!    прочитанной картой пула, максимумами вывода и суммы ордера (текст отказа точки сразу за наибольшим объёмом).
//!
//! Кто вызывает: core/card_max.rs (кривая ядра), тесты (tests/core_signal_max.rs — записанные стакан и пул CAKE).
//! От чего зависит: size.rs, size_search.rs, size_memo.rs.
//! Настройки: через Params (size.*, предел поиска — в Params.budget).

use rust_decimal::Decimal;
use serde::Serialize;

use super::size::{Curve, Fail, Params, Point};
use super::size_memo::Memo;
use super::size_search::search_in;

/// Итог поиска без бюджета.
#[derive(Debug, Clone, Serialize)]
pub struct MaxVolume {
    /// Оптимум прибыли без бюджета: объём и прибыль, USDT.
    pub opt_qty: Decimal,
    pub opt_profit: Decimal,
    /// Наибольший объём, проходящий пороги прибыли, и его прибыль; None — пороги не проходит ни один посчитанный.
    pub max_qty: Option<Decimal>,
    pub max_profit: Option<Decimal>,
    /// Списание на наибольшем проходящем объёме, USDT.
    pub max_spend: Option<Decimal>,
    /// Наибольший возможный объём поиска (до предела или до края площадок).
    pub upper_qty: Decimal,
    /// Верхний предел поиска (card.max_search_usdt), USDT.
    pub cap: Decimal,
    /// Упёрлись в предел card.max_search_usdt (иначе — в площадки, причина в limit).
    pub cap_reached: bool,
    /// Чем ограничен поиск сверху — человеческим текстом.
    pub limit: String,
    /// Сколько объёмов посчитано всего.
    pub evals: u32,
}

/// Что делает: ищет оптимум и наибольший проходящий объём без бюджета (см. шапку).
/// Принимает: кривую, параметры (budget = предел поиска), заведомо проходящий объём (из расчёта на бюджете; 0 — нет),
/// проверку порогов точки. Возвращает: MaxVolume. Ошибки: Fail поиска (Stop — расчёт невозможен; TooSmall/TooLarge —
/// ни один объём до предела не подходит).
pub async fn max_volume<C, F>(
    curve: &mut C,
    p: &Params,
    known_ok: Decimal,
    pass: F,
) -> Result<MaxVolume, Fail>
where
    C: Curve,
    F: Fn(&Point<C::Data>) -> bool,
{
    if p.step <= Decimal::ZERO {
        return Err(Fail::Stop(format!(
            "шаг количества {} не больше нуля",
            p.step
        )));
    }
    // Одни расчёты на поиск оптимума и бисекцию наибольшего объёма — точки не считаются дважды.
    let mut m = Memo::new(curve, p.step, p.budget, p.max_evals);
    let f = search_in(&mut m, p).await?;
    let upper = f.choice.max_qty;
    let hi = m
        .units(upper)
        .ok_or_else(|| Fail::Stop(format!("объём {upper} слишком велик")))?;
    let (cap_reached, limit) = edge(&mut m, p, hi, f.choice.max_exact).await;
    let found = biggest(&mut m, hi, known_ok, &f.best, p.bisect_steps, &pass).await?;
    let evals = m.evals();
    let (max_qty, max_profit, max_spend) = match found {
        Some(pt) => (Some(pt.qty), Some(pt.profit), Some(pt.invested)),
        None => (None, None, None),
    };
    Ok(MaxVolume {
        opt_qty: f.best.qty,
        opt_profit: f.best.profit,
        max_qty,
        max_profit,
        max_spend,
        upper_qty: upper,
        cap: p.budget,
        cap_reached,
        limit,
        evals,
    })
}

/// Наибольший проходящий объём бисекцией на [нижний проходящий, hi]. Принимает: расчёты, hi в шагах, заведомо
/// проходящий объём, оптимум, шаги, проверку. Возвращает: точку или None. Ошибки: Stop.
async fn biggest<C, F>(
    m: &mut Memo<'_, C>,
    hi: u64,
    known_ok: Decimal,
    opt: &Point<C::Data>,
    steps: u32,
    pass: &F,
) -> Result<Option<Point<C::Data>>, Fail>
where
    C: Curve,
    F: Fn(&Point<C::Data>) -> bool,
{
    let ok_at = |r: &Result<Point<C::Data>, Fail>| r.as_ref().is_ok_and(pass);
    let top = m.at(hi).await;
    if let Err(Fail::Stop(e)) = &top {
        return Err(Fail::Stop(e.clone()));
    }
    if ok_at(&top) {
        return Ok(top.ok());
    }
    // Нижний конец: больший из «проходящих» — оптимума (если проходит) и объёма на бюджете (не выше hi).
    let mut lo: Option<u64> = None;
    for q in [opt.qty, known_ok] {
        if let Some(n) = m.units(q).filter(|n| *n > 0 && *n < hi)
            && ok_at(&m.at(n).await)
            && lo.is_none_or(|l| n > l)
        {
            lo = Some(n);
        }
    }
    let Some(mut lo) = lo else { return Ok(None) };
    let (mut hi, mut left) = (hi, steps);
    while hi - lo > 1 && left > 0 {
        left -= 1;
        let mid = lo + (hi - lo) / 2;
        let r = m.at(mid).await;
        if let Err(Fail::Stop(e)) = &r {
            return Err(Fail::Stop(e.clone()));
        }
        if ok_at(&r) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(m.at(lo).await.ok())
}

/// Чем ограничен поиск сверху: точка на шаг выше наибольшего объёма. Принимает: расчёты, параметры, наибольший объём
/// в шагах, найден ли он точно. Возвращает: (упёрлись в предел поиска, текст). Ошибок нет.
async fn edge<C: Curve>(m: &mut Memo<'_, C>, p: &Params, hi: u64, exact: bool) -> (bool, String) {
    let next = m.qty(hi + 1);
    match m.beyond(hi + 1).await {
        Ok(pt) if pt.invested > p.budget => (
            true,
            format!(
                "предел поиска card.max_search_usdt = {}: объём {next} спишет {}",
                p.budget.normalize(),
                pt.invested.round_dp(4)
            ),
        ),
        Ok(_) if !exact => (
            false,
            "граница не уточнена до шага (мало size.bisect_steps)".into(),
        ),
        Ok(_) => (
            false,
            "объём выше не посчитан (предел size.max_evals)".into(),
        ),
        Err(e) => (false, format!("площадки: {e}")),
    }
}
