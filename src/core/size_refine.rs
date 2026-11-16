//! Файл: core/size_refine.rs
//! Что делает: уточнение одного локального максимума прибыли тернарным поиском (решение 66, п. 3 size_search.rs) и
//! ширина остановки уточнения. Вынесено из size_search.rs (задача t157: файл перерос 200 строк) без изменений.
//! Кто вызывает: core/size_search.rs.
//! От чего зависит: core/size_memo.rs, core/size.rs.
//! Настройки: size.refine_steps, size.tolerance_pct (через Params).

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use super::size::{Curve, Fail};
use super::size_memo::Memo;

/// Тернарный поиск максимума на [a, b] (целые шаги). Принимает: расчёты, концы, предел шагов, ширину остановки.
/// Ошибки: Stop. Недопустимый объём внутри считается хуже любого допустимого.
pub async fn refine<C: Curve>(
    m: &mut Memo<'_, C>,
    a: u64,
    b: u64,
    steps: u32,
    tol: u64,
) -> Result<(), Fail> {
    let (mut lo, mut hi, mut left) = (a, b, steps);
    while hi - lo > tol.max(2) && left > 0 {
        left -= 1;
        let m1 = lo + (hi - lo) / 3;
        let m2 = hi - (hi - lo) / 3;
        let (f1, f2) = (m.profit(m1).await?, m.profit(m2).await?);
        if f1 < f2 {
            lo = m1;
        } else {
            hi = m2;
        }
    }
    if hi - lo <= 2 {
        for n in lo..=hi {
            m.profit(n).await?;
        }
    } else {
        m.profit(lo + (hi - lo) / 2).await?;
    }
    Ok(())
}

/// Ширина остановки уточнения в шагах: tolerance_pct % от наибольшего объёма, не меньше одного шага. Ошибок нет.
pub fn tolerance(hi: u64, pct: Decimal) -> u64 {
    (Decimal::from(hi) * pct / Decimal::ONE_HUNDRED)
        .floor()
        .to_u64()
        .unwrap_or(0)
        .max(1)
}
