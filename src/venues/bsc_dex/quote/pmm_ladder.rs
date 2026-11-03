//! Файл: venues/bsc_dex/quote/pmm_ladder.rs
//! Что делает: котировка и глубина по «лесенке цен пула» (решение 122 F, 3; выбор Тиграна для WooFi): точки
//! (вход, выход) — ответы квотера самого контракта (`query`) на блоке по лесенке объёмов dex.pmm_ladder_usd. Своих
//! формул пула нет: в точках выход точный; между точками линейно интерполируется СРЕДНЯЯ цена (выход/вход), выход =
//! вход × средняя цена; ниже первой точки — средняя цена первой точки (меньший объём у пула не дешевле — оценка не
//! завышает выход); выше последней точки — не экстраполируем: объём неисполним. Откат квотера в точке (пределы
//! пула) — край лесенки (точек после него нет).
//! Глубина в полосе b: наибольший объём, при котором средняя цена не хуже цены первой точки на множитель полосы.
//! Кто вызывает: quote/pool_quote.rs, quote/depth_band.rs, quote/spot.rs (ветка OraclePmm); тесты.
//! От чего зависит: core/types (LadderPoint), math/full_math.rs (mul_div).
//! Настройки: нет (лесенку задаёт dex.pmm_ladder_usd при чтении — pools/load_woo.rs).

use alloy_primitives::U256;

use crate::core::types::LadderPoint;

use super::super::math::full_math::mul_div;

/// Масштаб средней цены (выход за единицу входа) — 10^36: хватает точности при любых decimals. Ошибок нет.
fn scale() -> U256 {
    U256::from(10u8).pow(U256::from(36u8))
}

/// Средняя цена точки (выход·10^36/вход). None — пустой вход. Ошибок нет.
fn rate(p: &LadderPoint) -> Option<U256> {
    if p.amount_in.is_zero() {
        return None;
    }
    mul_div(p.amount_out, scale(), p.amount_in)
}

/// Средняя цена на входе x внутри отрезка [a, b] линейно. None — отрезок вырожден или переполнение.
fn rate_at(a: &LadderPoint, b: &LadderPoint, x: U256) -> Option<U256> {
    let (ra, rb) = (rate(a)?, rate(b)?);
    let span = b.amount_in.checked_sub(a.amount_in)?;
    if span.is_zero() {
        return None;
    }
    let t = x.checked_sub(a.amount_in)?;
    if rb >= ra {
        ra.checked_add(mul_div(rb - ra, t, span)?)
    } else {
        ra.checked_sub(mul_div(ra - rb, t, span)?)
    }
}

/// Выход на точный вход x. Принимает: точки по возрастанию входа, x. Возвращает: выход; None — x больше последней
/// точки (неисполнимо), лесенки нет или точки не по возрастанию. Ошибок нет.
pub fn out(points: &[LadderPoint], x: U256) -> Option<U256> {
    let first = points.first()?;
    if x <= first.amount_in {
        return mul_div(x, rate(first)?, scale());
    }
    for w in points.windows(2) {
        if x <= w[1].amount_in {
            if x == w[1].amount_in {
                return Some(w[1].amount_out);
            }
            return mul_div(x, rate_at(&w[0], &w[1], x)?, scale());
        }
    }
    None
}

/// Глубина: наибольший вход, при котором средняя цена ≥ цена первой точки × factor (factor — доля, 18 знаков).
/// Возвращает: (вход, выход, true — граница найдена внутри лесенки; false — лесенка кончилась раньше полосы).
/// None — лесенки нет или расчёт невозможен. Ошибок нет.
pub fn depth(points: &[LadderPoint], factor_e18: U256) -> Option<(U256, U256, bool)> {
    let first = points.first()?;
    let lim = mul_div(
        rate(first)?,
        factor_e18,
        U256::from(10u8).pow(U256::from(18u8)),
    )?;
    let mut last = first;
    for w in points.windows(2) {
        let rb = rate(&w[1])?;
        if rb < lim {
            let ra = rate(&w[0])?;
            if ra <= lim {
                return Some((w[0].amount_in, w[0].amount_out, true));
            }
            // Средняя цена падает линейно от ra до rb: граница — где она равна lim.
            let span = w[1].amount_in.checked_sub(w[0].amount_in)?;
            let x = w[0]
                .amount_in
                .checked_add(mul_div(ra - lim, span, ra - rb)?)?;
            let y = mul_div(x, rate_at(&w[0], &w[1], x)?, scale())?;
            return Some((x, y, true));
        }
        last = &w[1];
    }
    Some((last.amount_in, last.amount_out, false))
}
